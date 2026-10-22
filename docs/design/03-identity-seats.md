# 03 身份、座位、代次、手动换舰长

文件：`gear/core/identity.py`（+ `cli/admin.py` 的 seat/captain 子命令）。预算 170–210 行。
守住的不变量：旧代次不能覆盖新状态；一条线一个续做控制者；身份不明不写。

## 概念

- **座位（seat）**：项目里的一个角色位置——舰长、某条长期线、某个临时工。座位持久，坐在上面的会话可以换。
- **代次（generation）**：座位每接入一次新会话 +1，旧绑定同时作废。
- **接入（onboard）**：GEAR 让一个 Agent 会话成为某座位的占用者的唯一方式。**MVP 中所有受管 Agent 都由 GEAR 启动**（新开或按 profile resume），不接管未经 GEAR 启动的现成会话。

## 固定调用上下文（替代"每次按 pane 查最新代次"）

接入时，引擎在目标 pane 的 shell 里先设置环境，再启动 Agent：

```text
pane run <pane> "export GEAR_CTX=<project>:<seat>:<gen>; export GEAR_DB=<state_dir>/gear.db"
agent start <name> --kind <kind> --pane <pane> -- <args 或 resume args>
```

Agent 进程及其所有子进程（包括它调用的每一个 `gear`）都继承 `GEAR_CTX`。Agent 不需要记任何参数。

接入目标必须是已核对的可用 shell pane：现有 Agent 尚未退出或结果不明时，不输入 export/启动命令，先完成旧会话停止与资源核对。环境设置与启动分步确认，`pane run` 返回不等于 shell 已执行完 export；环境继承须通过 15 的探针后才启用。

- 写请求时，`gear` 读取 `GEAR_CTX`，在事务内核对：座位存在、该代次 = 座位当前代次、绑定未作废、座位状态允许该操作。代次不符 → `STALE_GENERATION`。
- 旧会话派生的迟到命令带的是旧代次，换绑后一律被拒；`gear` **从不**按 pane 读取并采用最新代次。
- `HERDR_PANE_ID` 与只读 `herdr agent get` 指纹只作附加一致性检查（pane 对不上、占用者类型不符 → 拒绝），不作为身份来源。Herdr 查询在写事务**之外**完成，事务内只重读数据库绑定。
- 没有 `GEAR_CTX` 的调用：
  - 来自用户入口 `gear admin …`（用户在自己 shell 里执行；需 `GEAR_ADMIN=1` 或交互确认）→ 用户；
  - 其他 → 无角色，只读 `gear --all`，所有写入拒绝。**不因找不到座位就当成用户。**
- 迁移现有项目：现成 Agent 需由 GEAR 重新启动接入（支持 resume 的 CLI 用 resume 保留上下文）。

> 防的是误路由和旧会话误操作，不防同账号恶意伪造环境变量。

**事项引用**：
- `result` 和会改变任务状态的上报（含 `blocked`）必须明确关联原任务/尝试/合同（任务包给出完整命令），不从座位当前任务推断；同一会话做完 T-1 接 T-2 后，T-1 的迟到 result/blocked 仍归 T-1 原尝试。
- `ack` 按被回复事项的类型核对：任务相关 ACK（合同更新、插队）从该事项取关联并核对；**交接 ACK 只关联交接事项及其 pending binding/代次，不需要任务参数**；普通问题回复按原事项关联。
- 未关联的 `progress`、`question` 只作为座位级信息/问题，不归入任何尝试，也不改变任务状态。
- **待接班舰长**（pending binding）只能读取自己的交接包并提交该交接 ACK，其他写操作一律拒绝。

## 对外接口

```python
@dataclass
class Caller:
    kind: Literal["seat", "admin", "anonymous"]
    project: str | None
    seat: Seat | None
    generation: int | None
def resolve_caller(conn, env: Mapping[str,str], herdr_ro: HerdrReadOnly) -> Caller   # 查询在事务外
def require(conn, caller, *, roles=None, states=None) -> Seat   # 事务内重读并核对代次

def onboard_intent(conn, seat_id, pane_id, kind, profile, resume_ref: str|None) -> str
    # 事务内预留唯一新代次并创建 pending binding，返回 op id；重试/重启复用同一操作与代次
def onboard_complete(conn, op_id, fingerprint: Fingerprint) -> None
    # 只确认同一 binding 的启动结果，不再递增；普通座位确认后激活、旧绑定作废；
    # 待接班舰长仍保持 pending，直到针对本次交接的 ACK
def mark(conn, seat_id, state: SeatState, reason: str) -> None
```

数据（02 的 `seats` 与 `bindings` 表）：每次接入一条 binding（座位、代次、执行端/session、pane、kind、session_ref、状态 active/void/pending）。

## 座位状态迁移

```text
舰长:  ACTIVE → UNAVAILABLE（引擎检测，冻结权限；不自动恢复）
       ACTIVE/UNAVAILABLE → HANDOVER（用户发起）→ ACTIVE（新代次，待接班者确认后）
       UNAVAILABLE → ACTIVE 也必须经用户显式重新接入（同一会话也要新代次）
长期线: ACTIVE ⇄ STANDBY ⇄ PARKED；ACTIVE/STANDBY → HELD → ACTIVE；→ RETIRING → RETIRED；
       任意 → MISSING（引擎检测）→ NEEDS_CHECK（用户重新接入后，写入资源待核对）→ ACTIVE（用户确认）
临时工: QUEUED → STARTING → ACTIVE → RELEASING → RELEASED（Agent 已退出，pane 回池，可 resume）
       RELEASED → STARTING（resume 或新会话）；→ RETIRED
       STARTING/RELEASING 结果不明 → UNKNOWN（占额度，待核对）
```

RELEASED、QUEUED 的座位预期没有 Agent，08 不把它们判为 MISSING。

## 手动换舰长（MVP 唯一方式）

用户：`gear admin captain replace --project oxford --pane w5:p2 --kind codex [--resume <session_ref>]`

1. **一个事务内**：座位 → `HANDOVER`；旧绑定作废（由 onboard_intent 为新会话预留新代次，旧代次之后的写一律被拒）；记录待接班目标（pane、kind、本次交接版本）；对在途操作逐项分类写库：
   - INTENDED 未执行 → 保持，等新舰长确认后继续；
   - EXECUTING/UNKNOWN → 标"需核对"；
   - 舰长已回复、对方未确认的消息 → 保持原样；
   - 旧舰长 gear 之外的动作 → 引擎不推断，记"gear 外动作未核对"。
   交接中：`gear reply`、`gear plan apply` 对该项目拒绝；执行线 `gear send` 照常入库；已授权且不依赖交接结果的执行任务继续；**受交接影响或影响范围不明的操作保持冻结**。
2. 引擎按接入流程在目标 pane 启动新舰长（带新 `GEAR_CTX`，绑定状态 pending），生成交接包 `handover.md`（目标、计划版本、未处理消息、等待事项、最近 20 条决定、需核对操作、冻结项、下次复盘点、本次要求的 skill 版本），投递指针（与 skill 入口指针合为同一条投递，16）。交接 ACK 同时确认该 skill 版本。
3. **确认**：新舰长执行 `gear send --kind ack --reply-to <交接事项 ID>`。只接受来自待接班目标（pending 绑定的代次）对本次交接事项的确认。确认后原子激活：绑定 active、座位 ACTIVE、解冻舰长写入。确认不自动放行"需核对"的操作和写入资源，它们留在新舰长的首页。
4. 新 Agent 起不来或未确认 → 保持 HANDOVER，看板醒目显示；绝不存在两个有效舰长。旧舰长恢复后不自动复位。

## 舰长不可用（引擎检测，不自动换人）

检测：Herdr 报退出/关闭；屏幕连续命中额度/认证特征；有待处理事项、上次门铃已确认开始后仍持续无可证实处理。结果：座位 `UNAVAILABLE`，冻结舰长侧写入与派生操作，停止门铃，看板显示，`notify` 用户。限流类暂时状态只标注、不冻结代次（04 分类）。恢复只能由用户显式重新接入（原会话 resume 或新会话），走同样的核对。

## 测试要点

旧会话子进程在同 pane 换人后迟到写入被拒（同品牌、无 session_ref、查询后到事务前换绑三种）；无 GEAR_CTX 的 Agent 调用不变成 admin；交接中舰长写被拒、执行线 send 正常；只有待接班目标能确认；重启后交接状态可重建；UNAVAILABLE 不自动恢复。
