# 08 引擎循环、观测、副作用

文件：`gear/engine/loop.py`、`observe.py`、`ops.py`。预算 220–280 行。
守住的不变量：唯一执行 Herdr 副作用的进程；结果不明不重发；观察失败不当没事；单实例。

## 启动（`gear run`，由插件 pane 运行）

1. 单实例锁：`<state_dir>/engine.lock`（`fcntl.flock` 非阻塞）。拿不到 → 打印"引擎已在运行（pid、心跳、所在 pane）"后退出，不留第二个常驻进程。
2. 打开 DB、检查 schema 版本、读取所有项目配置并显示实际生效值。
3. **恢复核对**：所有 `EXECUTING` 的操作改为 `UNKNOWN`（上次进程死在执行中）；`SUBMITTED` 的投递保持待核对；不重发任何东西。核对数据库绑定的执行端/session 与当前 Herdr 一致，不一致拒绝运行。
4. 进入循环。

## 循环节拍

```python
def run_forever(conn, cfg):
    while True:
        t0 = now()
        snap = observe.snapshot(project_sessions)     # 每 session 一次 agent list
        for project in round_robin(projects):          # 项目轮转，单项目本轮有时间预算
            obs = observe.reconcile(conn, project, snap)   # 座位 ↔ Herdr 实况
            run_due(conn, project, obs)               # 09 续做/派活、07 门铃、10 人员、11 复盘、12 采样
        ops.drain_intended(conn, budget=…)            # 执行意图队列（有并发上限）
        write_heartbeat(conn); board.refresh(conn)
        sleep(max(0, cfg.tick - (now()-t0)))         # tick 默认 4 秒
```

- 低频任务：产出采样默认 20 分钟、复盘定时检查 1 分钟、过期 park/等待检查每轮。
- 一个项目本轮超时（例：Herdr 慢）→ 剩余工作推到下一轮，其他项目照常。
- 慢调用不卡主循环：`agent_start` 等长调用放在有界工作线程池（并发 ≤ 4）执行，主循环只登记和收结果；Herdr 调用和 git 采样都不持有 SQLite 写锁。

## 观测与对账（`observe.py`）

```python
@dataclass
class SeatObs:
    seat_id: str
    present: bool | None          # None = 观察失败
    status: str | None            # idle/working/blocked/done/unknown
    screen_flags: set[str]        # 04 classify_screen：busy/ratelimit/quota/auth（只在需要时读屏）
    fingerprint_ok: bool          # 占用者是否与绑定一致
    since: int                    # 本状态持续时间

def snapshot(sessions) -> dict[str, HerdrResult]
def reconcile(conn, project, snap) -> dict[str, SeatObs]
```

对账规则：

| 观测 | 动作 |
|---|---|
| agent list 调用失败 | 本项目本轮全部 `present=None`，不做任何判断，看板标"观察失败" |
| 座位 pane 不在 / Agent 退出 | 执行线（ACTIVE/STANDBY/PARKED/HELD/RETIRING）→`MISSING`（10 处理）；**舰长 → `UNAVAILABLE`**（03，冻结相关权限）；QUEUED、RELEASED、STARTING/RELEASING 中的预期变化按对应操作核对，不算失联 |
| 占用者指纹变了 | 先看是否有本次 onboard/release/交接操作：预期中的启动或退出按该操作核对；没有对应操作的意外变化才撤销绑定并 `NEEDS_CHECK`，不派活，通知 |
| 状态 idle/done | 读屏一次（尾部 N 行）确认无 busy 特征，再作为"空闲"交给 09 |
| 命中 ratelimit/quota/auth | 连续 `confirm_rounds` 轮才确认；舰长命中 quota/auth → 03 的 UNAVAILABLE |

## 副作用（`ops.py`）

每个副作用都走三步，全部写库：

```python
def intend(conn, project, seat, generation, kind, *, task=None, attempt=None,
           ref_msg=None, plan_version=None, payload=None) -> str   # INTENDED，意图时固定所需核对值
def execute(conn, herdr: HerdrActor, op_id) -> None   # EXECUTING → SUBMITTED / FAILED / UNKNOWN
def confirm(conn, op_id, evidence: str) -> None       # SUBMITTED/UNKNOWN → CONFIRMED（需关联证据）
```

- kind：`onboard`（启动并接入，含 resume）、`deliver`（派活/续做）、`doorbell`、`release`（让临时工退出、pane 回池）、`title`、`notify`。park/hold/retire 是纯数据目标，由 plan apply 在事务中完成，不进 ops。
- 执行前用意图时固定的代次、任务/尝试、计划版本核对当前状态；不一致 → `VOID`。
- `SUBMITTED` = Herdr 调用已返回成功，**不等于** Agent 已开始；`CONFIRMED` 需要与本次操作关联的证据：deliver/doorbell → 该座位当前代次的查看记录**实际展示了本次投递所指的内容**（13）；普通进展、相同尝试号的旧回合上报、其他详情查询都不算；onboard → 观测到预期 Agent 且指纹记录完成；release → 原 Agent 已退出且所需记录已保存；title/notify → 明确成功回执即可。working 本身不算证据。无法可靠关联时保持 UNKNOWN/待核对，不放宽。
- `UNKNOWN`（超时、进程被杀、回执不可解析）：不重发；核对预算内持续观察关联证据；超预算 → 座位标"需检查"并通知。
- 只有 Herdr 明确拒绝且已知未执行（如 `agent_blocked`）才标 `FAILED`。

## 测试要点

替身 Herdr：执行中被杀后重启不重发；观察失败不触发 MISSING；RELEASED 不判失联；占用者变化不继承；working 不确认投递；一个项目慢不阻塞另一个；单实例锁拿不到即退出。
