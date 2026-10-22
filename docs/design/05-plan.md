# 05 计划：草稿格式、应用、视图

文件：`gear/plan/draft.py`、`apply.py`、`render.py`。预算 270–330 行。
守住的不变量：单一权威计划；草稿不合法整份拒绝；完成必须有通过的验收结论；草稿不能覆盖运行事实。

## 文件位置（每个项目一个目录：`<state_dir>/projects/<id>/`）

| 文件 | 谁写 | 谁读 | 说明 |
|---|---|---|---|
| `plan.draft.md` | 引擎导出，舰长编辑 | 舰长 | 带 `base=<版本>` |
| `PLAN.md` | 引擎 | 舰长、用户 | 有界视图，只读 |
| `tasks/T-17.md` | 引擎 | 执行线 | 任务包，只读 |
| `handover.md` | 引擎 | 新舰长 | 仅交接时 |

**草稿不自动回写**：apply 成功后引擎不覆盖 `plan.draft.md`。需要新基准时，舰长用 `gear --show draft` 导出到新文件（不覆盖已有未提交的草稿文件）。PLAN.md 和任务包仍由引擎生成。

## 草稿格式（严格，一种）

```markdown
<!-- gear-plan project=oxford base=12 -->
# Goal
目标与完成标准。

# Milestone M2: 登录体系
范围；明确不做：……

# Decisions
- 2026-10-21 T-31 插队首：线上登录崩溃；T-17 在安全点后恢复

# Queue
## T-31 [now] lane=heat milestone=M2 required
goal: 修复 iOS 登录崩溃
accept: 复现用例通过；回归测试全绿
evidence-files: notes/T-31-repro.md
evidence-due: 60
deps: -

## T-17 [must] lane=wire milestone=M2 required
goal: …
accept: …
deps: T-12

# Staff
- temp R-4 task=T-45 profile=opencode-review
- temp R-2 task=T-31 resume
- park heat until=2026-10-21T15:30Z
- park media until-task=T-12
- hold wire reason=等用户授权
- retire prompts

# Verdicts
- T-29 attempt=2 contract=3 candidate=abc1234 pass evidence=logs/T-29-verify.md
- T-33 attempt=1 contract=1 candidate=def5678 fail rework="边界条件未覆盖，见 review.md"

# Review
V-9 keep plan=12: 依赖未解，维持顺序; next=2026-10-21T16:00Z
```

规则：

- `# Queue` 的块顺序决定 rank；`[now|must|should|could|wont]` 是独立的优先级字段，两者都不越过就绪门槛（依赖、容量、写入资源、授权）。未在草稿中出现的未终态任务**保持原状和相对顺序**（排在草稿中列出的任务之后），并在结果中列为"未提及"。删除只能写 `state=cancelled`。
- 允许字段：`goal`、`accept`、`evidence-files`、`evidence-due`（分钟）、`deps`、`state`、`followup-of`、`reopen`（仅 DONE→READY 时必填）；块头允许 `lane=`、`milestone=`、`required`。**任何未知字段或不允许的状态值 → 整份 `BAD_FORMAT`**。`state=` 只允许 `ready`、`cancelled`。
- 新任务写 `## NEW [should] lane=…`，应用时分配 `T-<n>`。
- **完成只经 `# Verdicts`**：每行指明任务、尝试、合同版本、候选。
  - `pass`：必须匹配该尝试提交的候选与已采用合同，且该合同等于本次事务拟应用后的当前合同；并有通过的证据（12 证据头，result=pass）。
  - `fail`：针对该待验证尝试原提交的候选与合同，给出 `rework=` 理由，不要求等于任务最新合同、不要求证据头；原尝试 REJECTED、任务 READY，下一尝试采用最新有效合同，旧结果与证据保留。旧合同候选可被明确退回，但不能完成新合同。
  - 已结算的完全相同 Verdict 再次出现只返回既有结果；同任务同尝试但内容冲突则拒绝；导出的新草稿不要求重交历史 Verdict。
- **已完成任务的同事项返工**：不对 ACCEPTED 尝试追加 fail、不覆盖历史结论。草稿中对 DONE 任务写 `state=ready` 并附 `reopen: <返工理由>` 字段，plan apply 校验后 DONE→READY，之后创建新尝试（临时工任务 resume 原临时工）。只有显式写出才重开；重放旧草稿、追问都不会重开。新活则建新任务和新临时工。
- `# Staff` 是**目标**而不是命令：同一 R 号/目标已实现或有在途动作时不重复生成操作；额度满时该项排队并在结果中提示，不拒绝整份计划。
- `resume` 只允许用于该 R 号原绑定任务的返工、追问、补材料；新任务必须新 R 号。`followup-of` 只表示任务关联，不授权复用旧临时工。 同一 resume 目标跨多次 apply 重复出现不重复恢复；同一 R 的新一轮返工/追问须关联新的返工尝试或追问事项，据此识别为新的恢复需求，而不是只比较字符串。
- `# Review` 每行关联复盘事项 ID 与所依据的计划版本，只关闭它列出的事项。

## 应用流程（`gear plan apply`，一个事务，经 run_request）

1. 解析 → 任何格式错误 `BAD_FORMAT`（附行号），不写任何东西。
2. 身份：舰长座位、代次一致、不在 HANDOVER/UNAVAILABLE。
3. `base` ≠ 当前计划版本 → `STALE_VERSION`，返回自 base 以来的变化摘要。
4. 校验：依赖存在且无环；wont 不派发；Verdicts 规则；resume 规则；park/hold/retire 只针对本项目长期线。
5. 写入：合同变化 → 新版本（在做尝试生成待采用通知，06）；rank；里程碑与 required；park/hold/retire 直接在事务中改座位状态（纯数据）；临时工启动/恢复写成 operations(INTENDED)；Verdicts → 06 `settle`；Review 结论 → reviews；计划版本 +1；audit。
6. 返回：新版本号、变化清单、待执行人员动作、排队项、未提及任务、警告。

## 视图

- `PLAN.md`：目标/里程碑/最近 10 条决定/全部在途任务/近期就绪前 15 条/紧急与受阻全部；其余写"另有 N 条，最老等待 X 天，`gear --show queue`"。≤ 250 行、12KB。
- 任务包 `tasks/T-17.md`：按**该尝试已采用的合同版本**渲染（有待采用更新时附在后面并标明）；首部列出可直接复制的命令：

```text
gear send --kind progress "…"
gear send --kind result --task T-17 --attempt 3 --contract 4 --candidate <commit> "报告：<路径>"
gear send --kind ack --reply-to <消息ID> …
```

## 对外接口

```python
def parse(text: str) -> Draft
def apply(conn, project: str, caller: Caller, draft: Draft, request_id: str) -> ApplyResult
def render_plan(conn, project) -> str
def export_draft(conn, project) -> str
def render_task_packet(conn, project, task_id, attempt_n) -> str
```

## 测试要点

格式错误行号；非法 state 整份拒绝；base 过期；依赖环；未提及任务保持相对顺序；Verdict 候选/合同不匹配被拒；证据文件存在但结论 fail 不准 DONE；resume 用于新任务被拒；相同 Staff 目标多次 apply 不双开；apply 后草稿不被覆盖；历史 Verdict 重复提交不重复结算；fail 不需证据头；同次改合同又验收按最终合同校验。
