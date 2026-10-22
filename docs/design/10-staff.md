# 10 人员：长期线待命、临时工（含 resume）、退役、失联

文件：`gear/engine/staff.py`。预算 240–300 行。
守住的不变量：一个写入资源一个写入者（不确定就不移交）；成果不丢；额度不被绕过。

## 长期线

| 状态 | 进入 | 行为 | 离开 |
|---|---|---|---|
| ACTIVE | 有在做的尝试 | 09 续做 | 交结果且无下一项 → STANDBY |
| STANDBY（待命） | 无可派任务 | **不发任何消息**；记录开始时间；进复盘快照；触发 11 的"有线空闲" | 有任务就绪 → 立即 assign |
| PARKED | 计划草稿 `park … until=/until-task=` | 不派活 | 到时间/条件满足 → 重新评估（有活派，无活 STANDBY）；新任务就绪也提前结束 |
| HELD | 草稿 `hold … reason=` | 不派活，新任务也不打断 | 只有草稿解除 hold |
| RETIRING | 草稿 `retire` | 不派新活；在途尝试按 06 插队流程到安全点；未开始任务退回待分配（lane 清空，舰长重派）；已提交的继续验收 | 交接完成 → RETIRED；舰长收到合并的复盘事项 |
| MISSING | 08 观测到预期有 Agent 的座位 pane/Agent 消失 | 不派活；在途尝试冻结；未开始任务可由舰长改派 | 用户登记新会话 → NEEDS_CHECK |
| NEEDS_CHECK | 占用者变化 / 失联后重新登记 | 不派活；worktree `writer_ok=0` | 用户 `gear admin seat confirm <seat>`（确认写入资源安全）→ ACTIVE |

**写入权**：MISSING/NEEDS_CHECK 的线所持有的 worktree，**不会**自动交给其他线；关掉 pane 不能证明后台作业已停。需要用户确认（`gear admin seat confirm`）后才能改派到别的线。MVP 规定受管任务不得创建脱离终端的写入作业（写入线说明）。

## 临时工

**额度与资源**：项目配置 `quota_temps`；STARTING、ACTIVE、RELEASING、UNKNOWN 都占额度；QUEUED、RELEASED 不占。启动在**一个事务内**同时预留额度和一个池槽位（`slots`：pane + worktree），并发启动不会拿到同一个空位。计划人数、会话额度、可安全使用的 worktree 数量分开计算；额度有余但没有空槽位 → QUEUED。

**启动**（草稿 `temp R-4 task=T-45 profile=…`，同一目标重复出现不重复启动）：

1. 事务：校验 profile 已验证（04）→ 预留额度与槽位 → 座位 STARTING → `intend(onboard)`。
2. 引擎：核对槽位 pane 的实际 cwd 是该槽位登记的规范化 worktree（不一致 → 槽位标 lost，座位回 QUEUED，通知用户）→ `pane_run` 设置 `GEAR_CTX` → `agent_start`。
3. 观测到预期 Agent → `onboard_complete`（03）记录指纹与 **session_ref**（profile 声明能拿到引用的，接入时就取；需在退出时才输出引用的 CLI，按 profile 的退出流程捕获）→ 首次任务包指针与 skill 入口指针合为同一条投递（16），不双发。启动超时/结果不明 → UNKNOWN（占额度），不另开。

**首次派发**：onboard 确认后，本模块为该临时工原绑定的 READY 任务调用 06 `start_attempt` 并登记首个 deliver；09 只对已建立的尝试做 continue，永不从队列领其他任务。

**正常交付后的自动释放**（全部满足才执行）：

- 临时工 `gear send --kind result --task … --attempt … --contract …` 已登记，并写明产物路径与安全结束点；
- 产物文件存在；需保留的改动已提交或有可核对的持久副本（只登记路径或脏树摘要不算）；
- 连续两轮观测 idle/done、屏幕无 busy、无已知未完成活动、无占位中的投递。

满足 → 座位 RELEASING → `intend(release)`：按 profile `exit_keys` 让 Agent 退出 → 确认 Agent 已退出且 session_ref 已保存 → shell pane **留在池里**（槽位回 free，worktree 归属记录"由 R-4 正常释放于 T-45"）→ 座位 RELEASED → 释放额度。任一条件不明 → 不关，给舰长"R-4 待核对"事项。释放额度不代表验收通过，也不把 worktree 交给其他人。

**resume 规则（用户决定）**：

- **新活 = 新临时工**（新 R 号、新会话）。
- **同一件活需要它回来**（验收不通过返工、追问、补材料）：草稿写 `temp R-2 task=T-31 resume`；只允许用于 R-2 原绑定的任务。
- 执行：事务内重新预留额度与槽位（优先原 worktree：若它的归属记录是本 R 号正常释放且之后无他人使用、无未知活动，直接复用；否则待核对）→ `intend(onboard, resume_ref=<session_ref>)` → `agent_start(args=resume_args(session_ref))` → 按 profile 的判定方式确认回到原会话 → 投递"返工说明"指针。
- 结果区分：
  - 明确不支持 resume（profile 无 resume）或确认恢复失败 → 新会话读原任务包 + 返工说明，座位 `note=新会话恢复`，首页和看板明确标出；
  - 启动超时、回执丢失、是否回到原会话不明 → UNKNOWN，**不另开新会话**，等核对。
- 没拿到 session_ref 的 profile 不得标为"resume 已验证"。
- **追问与返工分开**：对 VERIFYING/DONE 任务的追问，resume 后投递关联该任务的追问事项，不创建新尝试、不重跑实现、不改既有候选；追问处理完须重新声明安全结束点，不沿用上次交付的 safe_point 自动关闭。需要返工时：VERIFYING 任务走 `# Verdicts` 的 fail；DONE 任务走 `state=ready` + `reopen:`（05）；之后创建下一尝试并 resume 原临时工。**仅追问**时 VERIFYING/DONE 状态与原候选不变，回答后重新声明安全结束点，不需要也不允许伪造 result 或 fail 来关闭会话。
- 资源：`slots=free` 只表示 pane 槽位可用，不表示其中 worktree 可交给新任务；使用前事务性核对 resources。原树仍需保留且未安全释放时，新任务等待；原任务 resume 按原归属核对。

## 退役、换人后的花名册

首页与看板显示：计划长期线数、可用数、退役中/失联/待核对数及其未交接任务；临时工：在用/额度、排队、已释放可恢复列表。退役完成后才调整计划容量。

## 测试要点

待命不发消息、到期重评估、新任务提前唤醒；hold 不被打断；临时工超额排队；result 后仍在写不关；关闭未确认不释放额度；resume 用于新任务被拒；resume 失败退回新会话并标注；失联线的 worktree 不自动改派。
