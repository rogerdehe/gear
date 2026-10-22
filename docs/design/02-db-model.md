# 02 数据库与状态模型

文件：`gear/core/db.py`、`gear/core/model.py`。预算 280–340 行。
守住的不变量：单一权威状态；request 重放；旧代次不能覆盖新状态（与 03 合作）；一个写入资源一个写入者（resources 表）。

## 职责

- 打开每个执行端唯一的 SQLite（路径由已登记执行端/session 确定，见 14；WAL，busy_timeout 5000ms）。
- 建表与 schema 版本检查（只升不降；版本不符拒绝运行）。
- 事务、ID 分配、request 重放、错误码。
- 所有状态枚举与合法迁移表（`model.py`），其他模块写状态必须经 `transition()`。

## 不做

不做业务判断；不调用 Herdr；不做 ORM。Herdr 查询、git 采样都不在写事务内进行。

## 对外接口

```python
def connect(db_path: Path) -> sqlite3.Connection
@contextmanager
def tx(conn) -> Iterator[sqlite3.Connection]                 # BEGIN IMMEDIATE
def next_id(conn, project: str, kind: str) -> str            # T/M/R/V/O
def run_request(conn, request_id: str, caller_key: str, action: str, args: dict,
                fn: Callable[[], dict]) -> dict
    """args_hash = hash(项目, 调用者身份含代次, 动作, 规范化参数)。
    同 id 同 hash -> 返回原结果（不重新授权）；同 id 不同 hash -> REQUEST_CONFLICT；
    否则在同一事务执行 fn 并保存结果。"""
class GearError(Exception): code: str; detail: str

class TaskState(StrEnum):    DRAFT, READY, RUNNING, WAITING, VERIFYING, DONE, FAILED, CANCELLED
class AttemptState(StrEnum): DELIVERED, STARTED, STOPPED, SUBMITTED, ACCEPTED, REJECTED, ABANDONED, UNKNOWN
class SeatRole(StrEnum):     CAPTAIN, LANE, TEMP
class SeatState(StrEnum):    ACTIVE, STANDBY, PARKED, HELD, RETIRING, RETIRED, MISSING, NEEDS_CHECK,
                             UNAVAILABLE, HANDOVER, QUEUED, STARTING, RELEASING, RELEASED, UNKNOWN
class MsgKind(StrEnum):      PROGRESS, QUESTION, BLOCKED, RESULT, ACK, INSTRUCTION, REPLY, SYSTEM
class MsgState(StrEnum):     OPEN, WAITING, RESOLVED, INFO
class OpState(StrEnum):      INTENDED, EXECUTING, SUBMITTED, CONFIRMED, FAILED, UNKNOWN, VOID
def transition(kind: str, old: str, new: str) -> None
```

**request_id**：所有写入口（send、reply、plan apply、admin 写命令）都经 `run_request`。CLI 在执行前先生成 ID 并立刻打印到 stderr（`request: R-…`），再提交；也可由调用方 `--request-id` 指定。COMMIT 后进程死掉，调用方用该 ID 重跑拿回原结果。

## 表结构

```sql
CREATE TABLE meta      (key TEXT PRIMARY KEY, value TEXT);   -- schema_version, engine_heartbeat, engine_session
CREATE TABLE projects  (id TEXT PRIMARY KEY, herdr_session TEXT, config_toml TEXT, config_version INT,
                        quota_lanes INT, quota_temps INT, quota_includes_captain INT,
                        plan_version INT DEFAULT 0, goal TEXT, created INT);
CREATE TABLE milestones(project TEXT, id TEXT, title TEXT, scope TEXT, state TEXT, PRIMARY KEY (project, id));
CREATE TABLE seats     (id TEXT PRIMARY KEY, project TEXT, role TEXT, name TEXT, profile TEXT,
                        generation INT, state TEXT, state_since INT, state_reason TEXT,
                        park_until INT, park_task TEXT, hold_reason TEXT,
                        task_id TEXT,          -- 临时工绑定的任务（终身）
                        slot TEXT);            -- 临时工占用的池槽位
CREATE TABLE bindings  (seat TEXT, generation INT, session TEXT, pane_id TEXT, kind TEXT,
                        session_ref TEXT, status TEXT,    -- pending / active / void
                        handover_id TEXT, created INT, PRIMARY KEY (seat, generation));
CREATE TABLE resources (path TEXT PRIMARY KEY,           -- 规范化 worktree 路径（跨项目唯一）
                        project TEXT, holder_seat TEXT, state TEXT,   -- free / held / needs_check
                        last_task TEXT, note TEXT, updated INT);
CREATE TABLE slots     (project TEXT, id TEXT, pane_id TEXT, worktree TEXT, state TEXT,  -- free / used / lost
                        PRIMARY KEY (project, id));
CREATE TABLE plans     (project TEXT, version INT, draft TEXT, applied INT, by_seat TEXT, by_gen INT,
                        PRIMARY KEY (project, version));
CREATE TABLE tasks     (project TEXT, id TEXT, rank INT, state TEXT, priority TEXT, lane TEXT,
                        milestone TEXT, required INT, contract_version INT, deps TEXT,
                        followup_of TEXT, wait_on TEXT, wait_cond TEXT, rework_note TEXT,
                        updated INT, PRIMARY KEY (project, id));
CREATE TABLE contracts (project TEXT, task TEXT, version INT, goal TEXT, accept TEXT,
                        evidence_files TEXT, evidence_due_min INT, body TEXT, created INT,
                        PRIMARY KEY (project, task, version));
CREATE TABLE attempts  (project TEXT, task TEXT, n INT, seat TEXT, generation INT,
                        contract_version INT, adopted_version INT, pending_update INT,
                        state TEXT, candidate TEXT, report TEXT, safe_point TEXT,
                        started INT, updated INT, PRIMARY KEY (project, task, n));
CREATE TABLE messages  (project TEXT, id TEXT, thread TEXT, task TEXT, attempt INT,
                        sender TEXT, sender_gen INT, recipient TEXT, kind TEXT, urgent INT,
                        body TEXT, reply_to TEXT, needs_ack INT,
                        state TEXT, wait_on TEXT, wait_until INT, result TEXT,
                        created INT, resolved INT, PRIMARY KEY (project, id));
CREATE TABLE operations(project TEXT, id TEXT, seat TEXT, generation INT, kind TEXT,
                        task TEXT, attempt INT, ref_msg TEXT, plan_version INT,
                        payload TEXT, state TEXT, result TEXT, check_budget INT,
                        created INT, updated INT, PRIMARY KEY (project, id));
CREATE TABLE verdicts  (project TEXT, task TEXT, attempt INT, contract_version INT, candidate TEXT,
                        passed INT, evidence TEXT, by_seat TEXT, created INT);
CREATE TABLE reviews   (project TEXT, id TEXT, cause TEXT, state TEXT, detail TEXT,
                        decided TEXT, plan_version INT, checkpoint INT, reminders INT,
                        created INT, updated INT, PRIMARY KEY (project, id));
CREATE TABLE samples   (project TEXT, seat TEXT, task TEXT, ts INT, head TEXT, status_digest TEXT,
                        content_digest TEXT, complete INT, evidence TEXT);
CREATE TABLE reviews_of_work (project TEXT, task TEXT, ts INT, base_head TEXT, content_digest TEXT,
                        judgement TEXT, next_check INT, by_seat TEXT);
CREATE TABLE requests  (id TEXT PRIMARY KEY, args_hash TEXT, result TEXT, created INT);
CREATE TABLE audit     (ts INT, project TEXT, actor TEXT, action TEXT, detail TEXT);
```

计数类状态（冷却起点、续做次数、提醒次数、核对预算）都从 operations / attempts / reviews 的持久记录推出，引擎重启不重置。

## 合法迁移（摘要）

- **任务**：`DRAFT→READY`；`READY→RUNNING`（投递已确认开始）；`RUNNING→WAITING|VERIFYING`；`RUNNING→READY`（插队安全点已核对）；`WAITING→RUNNING`（恢复条件满足，继续原尝试）；`WAITING→READY`（仅当旧尝试已明确结算/终止、需要重新派发）；`VERIFYING→DONE`（验收通过）；`VERIFYING→READY`（验收不通过，带返工说明）；`DONE→READY`（仅由 plan apply 显式提出同事项返工并附理由，历史 ACCEPTED 与 Verdict 保留）；未终态 `→CANCELLED|FAILED`（仅计划草稿；运行中任务取消只停止新派发，不自动 ABANDONED 或释放资源，须核对安全点，未知继续占用）。
- **尝试**：`DELIVERED→STARTED|UNKNOWN`；`STARTED⇄STOPPED`；`STARTED|STOPPED→SUBMITTED`；`SUBMITTED→ACCEPTED|REJECTED`（仅验收）；`STARTED|STOPPED→ABANDONED` **只经**插队安全点核对或取消；`UNKNOWN→STARTED|STOPPED|SUBMITTED`（经关联证据核对恢复到已证实状态）；`UNKNOWN` 不能因等待超时直接 ABANDONED 或释放。
- **操作**：`INTENDED→EXECUTING→SUBMITTED（Herdr 调用已返回）→CONFIRMED（关联证据确认）`；`EXECUTING→FAILED`（明确未执行）`|UNKNOWN`；`INTENDED→VOID`（执行前核对已过期）；`UNKNOWN→CONFIRMED`（关联证据）`|FAILED`（核实未执行）。`SUBMITTED` 与 `UNKNOWN` 都占用该座位的唯一投递位置。
- 座位与消息迁移见 03、07。

## 错误码

`NOT_REGISTERED`、`STALE_GENERATION`、`STALE_VERSION`、`NO_TARGET`、`FORBIDDEN`、`BAD_FORMAT`、`BAD_TRANSITION`、`LOCKED`、`REQUEST_CONFLICT`、`QUOTA`、`UNCONFIRMED`（退出码 3）、`ENGINE_DOWN`（仅提示，不阻止写库）。

## 测试要点

迁移表穷举；request 重放三种情况且重放不授权新代次；并发 ID 不重复；锁超时不留半写；同一 worktree 不能被两个座位持有；重启后预算计数不归零。
