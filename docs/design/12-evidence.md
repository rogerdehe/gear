# 12 产出采样与验收

文件：`gear/work/evidence.py`。预算 110–140 行。
守住的不变量：完成绑定合同与产物版本；未知不当成功；文件变化只是线索。

## 产出采样（引擎每 20 分钟一次，每座位一次；不持 SQLite 写锁）

```python
def sample(worktree: Path, evidence_files: list[str], budget_bytes=2_000_000) -> Sample
def judge(prev: Sample|None, cur: Sample, task_view, now) -> list[Finding]   # 纯函数
```

采样分开记录三样（只用 git 子进程，超时 10 秒，不遍历全树）：

- `head`：`git rev-parse HEAD`；
- `status_digest`：`git status --porcelain` 的哈希（路径与状态）；
- `content_digest`：对 `git diff HEAD` 输出、未跟踪文件中**任务证据文件**及 `git status` 列出的未跟踪文件内容（有总字节上限）计算的哈希——同一个已修改文件被再次修改也能识别。超出上限或失败 → `complete=0`，不当作无变化。

| finding | 条件 | 结果 |
|---|---|---|
| `progress` | head / content_digest / 证据文件变化 | 记录；供 09 重置无进展计数 |
| `missing_evidence` | 超过合同 `evidence-due` 仍无 `evidence-files` 列出的文件 | 给舰长一个合并事项"T-17 缺少预期产出，需要核对" |
| `unreviewed` | 距上次复查 ≥ 2 小时且期间有变化 | 同上，"T-17 已 2 小时未复查" |
| `sample_failed` | git 失败 / worktree 不在 / 不完整 | 标观察失败，不推断停工 |

合同没写 `evidence-files/evidence-due` 的任务只做 progress/unreviewed，不猜期限。

**复查**：舰长 `gear --show T-17 --diff` 看到的是**当前工作区相对上次复查基线 HEAD 的 diff**（只存 HEAD 与摘要，不声称能恢复上次未提交快照）。舰长用 `gear reply <事项> --next-check <时间> "判断…"` 记录判断（写入 `reviews_of_work`：时刻、基线 HEAD、content_digest、判断、下次检查），需要纠偏时 `gear send --to <线> --kind instruction`。复查时若采样不完整或刚变化，提示重采。

## 验收

- 执行线 `gear send --kind result --task T-17 --attempt 3 --contract 4 --candidate <commit或产物版本> "报告：<路径>"` → 尝试 SUBMITTED、任务 VERIFYING（06）。
- 舰长自己验证或开临时审核工（10）。**证据文件必须带头部**：

```text
gear-verdict: task=T-17 attempt=3 contract=4 candidate=abc1234 result=pass
checks: <跑了哪些验证>
```

- 舰长在草稿 `# Verdicts` 写结论 → `plan apply` 校验（05）：尝试 SUBMITTED、候选一致、合同版本等于当前、pass 行：候选与已采用合同匹配、合同等于本次拟应用后的当前合同、证据头一致且 `result=pass`；fail 行：只针对该尝试原提交的候选与合同，给返工理由，不要求等于最新合同（见 05）；需要集成验收的任务还需集成证据。通过 → 06 `settle(passed)` → DONE。
- 不通过：`# Verdicts` 写 `fail rework="…"` → 尝试 REJECTED、任务 READY，返工说明进任务包；之后正常派发新尝试；临时工任务按 10 resume 原临时工。`gear reply` 只传达意见，不改变任务状态。
- 非代码任务：候选可以是报告/产物的固定版本引用，不强制 commit。
- 候选或合同变化 → 旧证据失效。
- 已完成任务的同事项返工：草稿 `state=ready` + `reopen:`，保留历史通过结论（05）。

## 测试要点

同一已修改文件再次修改能识别；未跟踪证据文件变化能识别；采样不完整不报"无变化"；缺证据只在合同有期限时触发；证据头 result=fail 不准 DONE；候选/合同不匹配被拒。
