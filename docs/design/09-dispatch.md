# 09 续做与派活判断

文件：`gear/engine/dispatch.py`。预算 140–180 行。全是纯函数，副作用经 08 ops。
守住的不变量：每线最多一个待核对投递；结果不明不重发；一条线一个控制者。

## 输入与输出

```python
@dataclass
class LaneView:
    seat: Seat
    obs: SeatObs
    attempt: Attempt | None           # 当前非终态尝试
    pending_op: Operation | None      # 该座位未确认的 deliver/doorbell
    last_deliver_at: int | None
    nudges_since_progress: int        # 自上次可证实进展以来的续做次数
    ready_tasks: list[Task]           # 该线就绪队列（rank 序）

class Action(NamedTuple):
    kind: Literal["continue", "assign", "none", "escalate"]
    task: str | None
    reason: str

def decide(v: LaneView, cfg: DispatchCfg) -> Action
```

## 判断顺序（`decide`）

1. 座位不是 ACTIVE/STANDBY/RETIRING（PARKED、HELD、MISSING、NEEDS_CHECK、UNAVAILABLE、HANDOVER…）→ `none`。
2. 观测为 None（观察失败）→ `none`。
3. 该座位有任何占位中的投递（deliver/doorbell 处于 INTENDED/EXECUTING/SUBMITTED/UNKNOWN）→ `none`（等核对，不重发）。
4. Agent 不空闲（working / blocked / 屏幕有 busy）→ `none`；blocked 交 10 记为需人处理。
5. 屏幕确认 ratelimit / quota / auth → `none`，原因写入座位（10 处理），不催。
6. 距上次投递 < 冷却（默认 90 秒，启动时校验 > tick）→ `none`。
7. **三类动作**：
   - 有当前尝试且状态 STARTED/STOPPED（未交结果）→ `continue`（同一任务同一尝试）；若 `nudges_since_progress ≥ 预算`（默认 5）→ `escalate`（给舰长一个事项"T-17 连续续做无进展"，不再催）。
   - 当前尝试 SUBMITTED（等验收）→ 不重复实现；只有满足 06 的"领下一项安全条件"才 `assign` 下一条，否则 `none`。
   - 无在做尝试：有就绪任务 → `assign` 第一条；无 → `none`（10 置 STANDBY）。
   - `continue` 额外要求：任务不在 WAITING、未被 hold、无未解决阻塞、未在待验证。
   - `assign` 前（执行时在事务内重读）：任务 READY、非 wont、依赖按版本满足、座位状态与代次、合同、写入资源由本座位持有、无在做/UNKNOWN 尝试。
   - 临时工只会 `continue` 自己绑定的任务，永不 `assign`。
   - RETIRING：禁止 `assign`，但允许对现有尝试发安全结束/交接相关的 `continue`。

## 投递内容（模板，带版本，校验不含任何屏幕 marker）

- continue：`继续 T-17（合同 v4）。任务包：<路径>。有进展/受阻/完成用 gear send。`
- assign：`新任务 T-31（合同 v1）。任务包：<路径>。先读再开始；汇报用 gear send。`
- 规则只在线说明和任务包里，不随催办广播。

## "可证实进展"的判定（重置 `nudges_since_progress`）

新 commit、工作区内容变化、证据文件更新（12 的采样结果）、带可核对证据引用的 result。普通 progress 文字和屏幕输出不清零计数。

## 舰长座位

同样走 `decide`，但没有任务队列：有待处理事项或到期复盘 → 由 07 门铃 / 11 复盘提醒负责，不发 continue。

## 测试要点（含旧 keeper 事故回归）

- 催办词包含限流 marker → 模板加载时拒绝。
- 状态 done 但屏幕 busy → 不催。
- 冷却 ≤ tick → 启动时报错。
- 无新 commit 但仍在停轮 → 照常按冷却续做，不因 HEAD 不变永久停止；超预算升级。
- 占位中的投递存在时绝不再投递（含冷却已过）。
- progress 刷屏不清零无进展计数。
