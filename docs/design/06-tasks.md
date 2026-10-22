# 06 任务与执行尝试

文件：`gear/work/tasks.py`。预算 130–160 行。
守住的不变量：一个写入资源一个写入者（与 10、resources 表合作）；完成绑定合同与产物版本；旧尝试不能完成新合同。

## 职责

- 任务与尝试的状态变化（按 02 迁移表）。
- 一个任务同一时刻最多一个非终态尝试；一个座位同一时刻最多一个**在做**的尝试（DELIVERED/STARTED/STOPPED/UNKNOWN）。
- 合同版本切换、插队安全点、验收结算。

## 不做

不判断何时续做（09）；不解析计划草稿（05）；不跑验证（12）。

## 对外接口

```python
def current_attempt(conn, project, task) -> Attempt | None
def start_attempt(conn, project, task, seat_id, generation) -> Attempt
    # 前置（事务内重读）：任务 READY；依赖按版本满足；座位 ACTIVE/STANDBY 且代次一致；
    # 座位无在做尝试；该线的 worktree 由本座位持有且无未保存的他任务改动（见下）
def on_delivery_confirmed(conn, attempt) -> None        # 任务 READY→RUNNING，尝试 →STARTED
def on_stopped / on_started(conn, attempt) -> None
def submit_result(conn, project, task, attempt_n, contract_version, candidate, report, safe_point) -> None
    # gear send --kind result；必须引用确切任务/尝试/合同；尝试→SUBMITTED，任务→VERIFYING
def adopt_contract(conn, project, task, attempt_n, update_msg_id, version) -> None
    # 只接受针对已发送的合同更新事项的明确回执，版本须与该事项一致；不得降低已采用版本；
    # 期间又有更新则新更新保持待采用
def preempt_request(conn, project, task, reason) -> str  # 生成"安全点停下"指令消息（needs_ack）
def preempt_ack(conn, project, task, attempt_n, instruction_id, saved_ref) -> None
    # 回执须关联确切指令、任务、尝试、版本，并给出成果保存引用（commit 或已登记的持久副本）；
    # 核对后：尝试→ABANDONED，任务→READY（负责线不变）
def settle(conn, verdict: Verdict) -> None               # 只由 05 plan apply 的验收结论调用
```

## 关键规则

- **合同版本**：plan apply 改了合同 → 新版本；若有在做尝试，生成 needs_ack 的 SYSTEM 消息"T-17 合同已更新到 v4"。尝试继续绑定 `adopted_version`（任务包按已采用版本渲染，并标出"待采用 v4"），直到 `adopt_contract`。用旧版本提交的结果保留为事实，但**不能完成新合同**（验收按任务当前合同核对）。
- **领下一项的安全条件**：SUBMITTED 只表示候选已登记。长期线领下一项前必须：原尝试已到安全结束点、成果已有持久副本（已提交或可核对的持久副本）、无已知未完成活动或待核对投递、新任务基线已核对（见下）。否则等待，由舰长决定。临时工始终绑定自己的任务，永不领队列其他任务。
- **插队**：先 `preempt_request`，等执行线 `ack --preempted` 并核对保存引用后才转 ABANDONED；登记脏树只用于保留成果，不授权在同一树混写新任务。MVP 没有可安全隔离的资源时，新任务等待或由舰长改派其他线。
- **验收**：`settle(passed)` → 尝试 ACCEPTED、任务 DONE；`settle(failed)` → 尝试 REJECTED、任务 READY 并保存返工说明；之后正常派发创建新尝试（临时工任务按 10 的 resume 路径找回原临时工）。

- **依赖等待**：普通等待保留同一尝试；恢复条件满足后 WAITING→RUNNING，继续原尝试。只有旧尝试已明确结算/终止并需要重新派发时才回 READY。
- **领下一项的基线**：工作区干净或改动已提交只证明成果已保存，不证明新任务基线已隔离。派发下一项须核对新任务认可的基线，不能把旧任务未验收的提交隐式当作新任务输入；没有安全资源时等待或改派，不自动搬迁。
- **临时工**：onboard/resume 完成后，由 10 为其原绑定的 READY 任务调用 `start_attempt` 并登记首个 deliver；READY 但已有 DELIVERED/UNKNOWN 尝试的任务不再首派第二次。

## 测试要点

座位两个在做尝试被拒；已提交但工作区不干净时不领下一项；旧版本结果不完成新合同但原文可查；adopt 降版本被拒；ack 不带保存引用被拒；验收失败回 READY 后新尝试正常。
