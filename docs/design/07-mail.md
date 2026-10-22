# 07 消息：发送、收件箱、回复、门铃

文件：`gear/mail/send.py`、`gear/mail/inbox.py`、`gear/engine/doorbell.py`。预算 250–310 行。
守住的不变量：消息逐项确认；发送成功=已入目标收件箱；领取/已读/门铃都不算处理；每线单待核对投递（与 09 共用）。

## 发送（`gear send`，经 run_request）

```python
def send(conn, caller: Caller, kind: MsgKind, body: str, *,
         task: str|None, attempt: int|None, contract: int|None,      # result/ack 必填，来自任务包
         about: str|None, to: str|None, reply_to: str|None, urgent: bool,
         candidate: str|None, preempted: str|None, saved_ref: str|None,
         request_id: str) -> SendResult
```

1. 身份（03，固定调用上下文）；事务内核对代次。待接班舰长只允许提交交接 ACK。
   参数按类型校验：`result`、`blocked` 必须带原任务/尝试/合同；`ack` 从 `--reply-to` 的事项取关联（交接 ACK 不要求任务参数）；未关联的 progress/question 记为座位级。
2. 收件人：
   - 执行线默认 → 本项目舰长座位；
   - `--about T-x`：T-x 必须是本任务合同里已登记的依赖 → T-x 当前负责线；无负责线或非已登记依赖 → 舰长。路由本身不创建依赖。
   - `--to <座位>`：仅舰长（kind=INSTRUCTION）；
   - `--reply-to M-x`：回给 M-x 的发件人座位（若该座位已换代，送到当前占用者并标注）。
   - 舰长 HANDOVER 中：照常入库，门铃暂停。
3. 线程：`reply_to` 继承原线程；新问题开新线程 `<任务>:<序号>`；progress 归任务进展流。
4. 状态：progress → INFO；question/blocked/result/instruction → OPEN；**ack/回复**见下。
5. 附带动作（同一事务）：
   - `result` → 06 `submit_result`（必须引用确切任务/尝试/合同）；
   - `ack --reply-to <合同更新事项>` → 06 `adopt_contract`；
   - `ack --preempted T-x --saved <引用>` → 06 `preempt_ack`；
   - `ack --reply-to <交接事项>` → 03 交接确认（同时确认交接引用的 skill 版本）；
   - `ack --reply-to <skill 更新事项>` → 16 记录该座位当前代次采用的 skill 版本；
   - `blocked --about T-x --need "<接口/产物/决定及版本>"` → 任务 WAITING，记录等待对象与恢复条件。
6. 返回：`已送达 <收件人> 收件箱 M-88（线程 T-31:2）` + 门铃状态（"待对方空闲时提醒；引擎最近心跳 10:32:05" 或 "引擎未在线，恢复后提醒"）。

### 执行线如何"处理完"收到的事项

- 普通 REPLY（答复）为 INFO，不要求再确认，避免互相确认的死循环。
- 带 `needs_ack` 的事项（合同更新、插队指令、交接事项、需执行的 INSTRUCTION）保持 OPEN，直到收件人用 `gear send --kind ack --reply-to <ID>` 给出有效回执并通过对应校验；有效回执只关闭列出的原事项，回执本身记为 INFO。

## 收件箱选择（舰长首页调用）

```python
def select_batch(conn, project, recipient, *, max_threads=6, per_sender=2) -> list[ThreadView]
```

1. 候选：OPEN 与到期复核的 WAITING。
2. 若存在普通线程积压，**先预留 1 个名额**给最早未处理的普通线程。
3. 其余名额：含紧急消息的线程优先，同级按最早未处理时间；同一发件人最多 `per_sender` 个线程（含预留的那个）。
4. 每个线程：未处理原文（最多 3 条，超出显示数量）、最近一条回复/决定、所属任务一行摘要；`gear --show T-31:2` 展开全部。
5. INFO 进展只在简报中每线汇总一行。

## 回复（`gear reply`，舰长，经 run_request）

```python
def reply(conn, caller, ids: list[str], text: str, *, mode: Literal["resolve","wait","note"],
          wait_on: str|None, wait_until: int|None, downgrade: bool,
          next_check: int|None, request_id: str) -> ReplyResult
```

- 只处理列出的确切 ID（本项目、发给舰长、OPEN/WAITING）。
- `resolve`：→RESOLVED 并记录结果；`wait`：→WAITING，记录等谁、何时复核；`note`：只回复不改状态。
- 只带 `--downgrade` 时默认 `note`：只调紧急度，保持 OPEN/WAITING；只有显式 `--resolve` 才关闭。`--next-check` 只有在确实完成了本次复查时才可与 resolve 一起用，仅改时间不算完成复查。
- 回复写进原发件人收件箱（同线程，INFO；若是需执行的指令则用 `gear send --to` 发 INSTRUCTION）。
- `--next-check` 用于产出复查（12）：记录复查判断与下次检查时间。
- 回复不完成任务、不改合同、不移交写入权、不改变任务/尝试状态（这些只经 plan apply）。

## 等待的解除

WAITING 的任务/事项在以下情况由引擎重新评估：等待的上游任务验收通过且产物版本满足；关联事项被有效回复；到复核时间。任意回复或一句"做完了"不解锁。

## 门铃（引擎）

```python
def due_doorbells(snapshot) -> list[DoorbellAction]   # 只读评估：调用层先查好状态再传入
```

- 条件：收件人有 OPEN 或到期 WAITING 事项，**或有尚未展示的新 REPLY/INSTRUCTION**（依查看记录判断）；收件人观测为空闲（idle/done 且屏幕无 busy）；该座位**没有任何占位中的投递**（INTENDED/EXECUTING/SUBMITTED/UNKNOWN 的 deliver 或 doorbell）；距上次门铃已确认结束 ≥ 冷却。普通 INFO 回复展示后不再按铃、无需 ACK；progress 不逐条按铃；needs_ack 事项即使已展示仍保持 OPEN 直到有效回执。
- 内容一行：`你有 5 个待处理事项（2 个紧急），先运行 gear`；复盘到期时改为"先处理复盘"（11）。
- 投递后操作处于 SUBMITTED；只有**关联本次门铃的查看记录**（13：该次首页实际展示了门铃所指的事项 ID）才 CONFIRMED。没有相关证据：不重发；超过核对预算（默认 3 个 tick 周期 × 冷却）→ 座位标"需检查"并通知。working 本身不算确认。

## 测试要点

十条处理一条；同任务两问只答一；紧急洪峰下预留的普通名额不被同发件人挤掉；单发件人刷多线程被限；拉取后新到不被关闭；回复不触发无限确认；ack 只关闭列出事项；交接中入库不按铃；门铃未确认冷却后也不重发；COMMIT 后丢回执重放。
