# Orca 的项目编排能力与可借鉴的故障防护

研究目的：学习 Orca 已经实现的工作组织与故障处理，减少重复踩坑。本文不推荐用户迁移到 Orca，也不把其 GUI、运行时或组织约定直接当成我们的需求。

固定来源：`stablyai/orca@76f9978880752f2b3ed63bb8c2698b8a7236440d`。本次读取官方仓库的协调指南、核心源码、看板源码及回归测试断言；未运行 Orca 应用、未安装其完整工具链、未执行其上游测试。源码版本、已取文件及校验值见 [orca-sources.json](orca-sources.json)。这里的“已确认”指实现/合同存在，不能解读为生产实测通过。此前只看 README 不足以完成用户要求，本次补查的是遗漏的 orchestration 层。

**1. 回答用户的三个直接问题**

| 问题 | 结论 | 实际依据与边界 |
|---|---|---|
| Orca 有没有看板？ | 有，而且至少有两类不同看板 | Agent 看板按 Needs You / Working / Done / Idle 展示快照；Workspace 看板支持状态列、移动、排序和可选 Linear 状态同步。不能把这两种 UI 状态当作 Task-DAG 验收结果 |
| Orca 有没有派活？ | 有结构化编排 | Run、Task、Dispatch、worker-start、依赖、ready 查询、决策 gate、消息与显式结果；并非只向终端发送文字 |
| 是否处理 Agent 停止、提醒和恢复？ | 有明确的监督/恢复机制 | 分离 PTY/Agent 存活、提示交付、worker_done、失联未知与清理归属；协调 loop 负责持续检查，进程退出可向 Run 收件箱升级并唤醒等待者。没有证据可把它概括为“后台会无条件让任意 Agent 永远继续” |

**2. 实际组织模型**

Orca 的编排指南把三个对象分开：Run 是持久命名空间和 coordinator 收件箱；Task 是工作；Dispatch 是一项工作的一次权威执行尝试。Run 本身不负责排程或放置 worker。

协调者先创建独立任务波次，记录真实依赖，再启动 worker。支持 `task-list --ready` 找可执行任务，`worker-start --task` 启动已有任务，重试使用同一 Task 和 `--retry-of` 指向原尝试。完成报告必须带当前 Task/Dispatch 身份和明确 succeeded/failed 结果。

一个有效 worker_done 会结算 Task 与 Dispatch；因此“结算是由谁报告的”和“是否验证了工作内容”需要分别判断。我们的产品要求在完成声明后有对应验收/审核证据，不能直接把一条成功声明当作最终交付。

[协调合同](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration.md)、[DAG 与复用](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/coordinator-loop.md)

**3. 看板究竟在管什么**

`AgentKanbanBoard.tsx` 从 DashboardSnapshot 分组，包含 attention、working、done、idle 四列，按状态变化时间排序，可以打开对应终端。它表达 Agent 当前状态与需要关注的事项。

工作区看板是另一个表面。`use-workspace-kanban-status-actions.ts` 管理状态名称、图标、颜色、列顺序，以及删除状态后的工作区归属；更新 worktree 元数据时带 executionHostId。`WorkspaceKanbanDrawer.task-status-sync.test.tsx` 对拖动/菜单变更后的 Linear 同步有断言，同时明确“只改手工顺序、没有改状态”不触发状态同步。

因此可借鉴：从真实快照投影 UI；保留执行主机身份；区分看板操作的含义；同步失败需要显示。**本次没有证明其工作区卡片排序就是 orchestration 的实际任务派发优先级，不能宣称现成功能已经覆盖用户需要的每 Agent 后续任务队列。**

[Agent 看板源码](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/renderer/src/components/dashboard-popout/AgentKanbanBoard.tsx)、[工作区动作](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/renderer/src/components/sidebar/use-workspace-kanban-status-actions.ts)、[同步测试](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/renderer/src/components/sidebar/WorkspaceKanbanDrawer.task-status-sync.test.tsx)

**4. 已编码的故障防护及我们的要求映射**

以下是明确可读到的合同、实现或测试场景。有回归测试不代表我们知道每项历史事故的完整经过，因此不把所有防护都编造成“线上发生过的事故”。

| 编号 | 故障模式 | Orca 的处理 | 纳入 PRD |
|---|---|---|---|
| O-01 | 两个派发者同时认领同一任务/终端 | Task/Dispatch 事务和占用检查，败于竞争的操作回滚/拒绝 | FR-02/04：唯一写入尝试，原子派发 |
| O-02 | 查询依赖时未完成，插入任务时依赖已完成 | 创建与状态核对测试覆盖这个竞态；失败/blocked 的依赖不能解锁下游 | FR-04：真实就绪条件，避免永远卡在 pending |
| O-03 | 提示已粘贴，但没观察到新回合启动 | 标为 dispatched-unobserved；不重新粘贴、不把任务放回 ready；迟到报告仍可正确结算 | FR-05：交付不确定不等于没交付 |
| O-04 | 收到消息后处理者崩溃 | 持久 Delivery 按 FIFO 重放，处理后才 ack；类型过滤只决定何时唤醒，不允许跳过旧消息 | FR-07/12：消息不丢，处理可恢复 |
| O-05 | 旧 worker 恢复，试图确认新 worker 的消息 | 接管增加 consumer generation，旧消费者收到 consumer_fenced，新消费者获得未读消息 | FR-03/12：防止旧会话越权和消息被误吞 |
| O-06 | 启动失败，每次重试又创建一个活进程 | 跨 retry-of 计算启动失败，第三次熔断并拒绝第四次；worker 自己的业务失败不按同一计数处理 | FR-05：区分启动失败和任务失败，设置恢复预算 |
| O-07 | PTY 还活着，Agent 实际已死或停住 | fleet liveness 与 worker-show 的 PTY observation 分层，不能以终端存活代替执行者存活 | FR-05/06：多层状态与证据来源 |
| O-08 | worker 退出，协调者仍在等待 | 退出升级写入所属 Run mailbox，测试覆盖唤醒 check --wait、仅通知正确 Run，以及 coordinator 尚未绑定的情形 | FR-07/12：退出不能只留日志，要有待处理事项 |
| O-09 | 遥控端失联就开始在本地清理/重启 | execution host 拥有实际事实；失联为 unverifiable；操作按 Dispatch 路由，不推测远端 terminal handle | FR-12：未知不等于死亡，不跨主机误操作 |
| O-10 | 释放旧任务终端时，它已经被复用或用户接管 | release 检查结算、所有权、进程 incarnation 与归档；不清理 pre-existing、setup、coordinator、active、user-taken-over 终端 | FR-10：清理必须基于资源归属，不按名字匹配 |
| O-11 | 删除 worktree 顺便删掉未合入 commit | 测试要求安全 branch -d；未合入时保留分支及 head，不升级成 branch -D | FR-02：会话、目录、分支分别回收 |
| O-12 | 请求超时后再调用，造成重复 mutation | request ID 与 request-show 恢复已完成/pending 状态；absent 也不自动证明从未执行 | FR-05/12：重试必须核对操作回执 |

对应源码与测试：

- O-01：[Task/Dispatch 不变量](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db-task-dispatch-invariant.test.ts)、[并发竞争](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db-task-dispatch-races.test.ts)。其中覆盖“旧失败不能覆盖已经完成的 worker 报告”。
- O-02：[依赖就绪竞态](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db-task-create-readiness.test.ts)。
- O-03：[派发实现](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/coordinator-task-dispatch.ts)、[未观察到启动的测试](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/coordinator-dispatch-unobserved-prompt.test.ts)、[启动记录修正](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/worker-start-unobserved-prompt-settlement.test.ts)。
- O-04：[持久投递合同](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/messaging-and-gates.md)、[mailbox 实现](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/messages/role-mailbox-delivery.ts)。
- O-05：[旧消费者撤权测试](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/dispatch-mailbox-consumer-fencing.test.ts)。
- O-06：[失败启动熔断测试](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/worker-dispatch/failed-start-circuit-breaker.test.ts)。计数语义不能直接拿来作为所有业务修复的上限。
- O-07：[恢复指南](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/recovery-and-cleanup.md)、[需关注事项分类](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/shared/orchestration-fleet-attention.ts)。
- O-08：[退出升级测试 STA-4604](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/lightweight-run-worker-exit-escalation.test.ts)。
- O-09：[远程放置与能力协商](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/placement-and-remote.md)。
- O-10：[资源归属](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/worker-terminal-ownership.ts)、[release 事务](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/runtime/orchestration/db/worker-terminal/worker-terminal-release.ts)。
- O-11：[移除 worktree 与保留分支测试](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/main/git/worktree-remove-branch-deletion.test.ts)。
- O-12：[mutation 恢复合同](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/recovery-and-cleanup.md)。另外 `RemoteRuntimeTerminalCreateIdempotency` 只合并进行中的相同创建请求，完成即移除内存项；不能把这一小段代码宣传为跨重启持久幂等。

**5. 提醒与 keep-live：明确已经有什么，仍需验证什么**

worker 合同要求在自然检查点检查 follow-up，并在 worker_done 前再查一次；完成后结束当前回合，不能自己启动无关工作。coordinator 等待结果和问题、处理后 ack；连续空等待后枚举 worker，依据 liveness、attention 与 nextAction 检查。

消息成功 enqueue 只证明持久入队；唤醒/nudge 是 best-effort，不证明对方读到或执行。`ask` 超时保留原问题，使用原 message ID 恢复。心跳说明存活，不说明完成。

`check-keepalive.ts` 定时向 stderr 输出 `_keepalive`/`_heartbeat`，说明等待命令仍活着；它没有启动新的 Agent 回合。`coordinator-task-dispatch.ts` 有心跳陈旧警告，但警告本身也不是自动恢复。

可借鉴的是：精确定义状态、让异常进入持久收件箱、提供恢复路径，并避免盲目重送。本产品的“未完成而正常停轮自动续做、主控自己停轮也继续受监督”仍须单独做端到端验证，不能从 heartbeat 这个字段推断已经成立。

[worker 合同](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/skill-guides/orchestration/references/worker-contract.md)、[等待命令 keepalive](https://github.com/stablyai/orca/blob/76f9978880752f2b3ed63bb8c2698b8a7236440d/src/cli/handlers/orchestration/check-keepalive.ts)

**6. Worktree 经验与 Herdr 复用边界**

Orca 区分新 Agent 终端、新 worktree、当前/已有工作区、独立顶层 worktree 和叠加子 worktree；仓库 setup 可有 start-immediately 或 wait-for-setup 策略。远程的 current/new-child 因含义不明确而拒绝，需要准确远程选择器。

其指南默认可使用现有工作区，不要求每个 worker 都创建 worktree。我们的产品按用户要求更严格：并行写代码默认独立 worktree。借鉴的是创建/setup/归属/回收的完整生命周期，而不是照搬它的共享默认值。

Herdr 已有 worktree list/create/open/remove 的 CLI/API，创建参数包含 branch/base/path/cwd/workspace；打开操作校验 checkout 身份、复用已打开 workspace，并返回 workspace/tab/root pane 与 worktree 信息。脏工作树删除有明确处理。应直接评估和复用这些原语，再补业务层的唯一写入权、任务关联、验收与清理策略。

Herdr 本体能力依据：[worktree schema](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/api/schema/worktrees.rs)、[worktree/workspace 集成](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/app/api/worktrees.rs)、[Git 操作与删除检查](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/src/worktree.rs)。本次没有操作用户现有 Herdr 会话。

**7. 对下一步实施的具体约束**

先以 [PRD](../../PRD.md) 为主，不再从“哪个工具看起来不错”反推用户需求。落地讨论必须逐项回答：

1. Herdr 原语直接复用哪些？业务状态由谁持久维护？
2. Task、attempt、会话、worktree、消息回执如何一一关联？
3. 如何避免 stale completion、双派发、丢 ack、失联误杀？
4. 看板的移动/排序究竟改变哪个事实，是否真的影响队列？
5. 哪个外部进程在模型停轮后继续负责？如何证明它本身仍活着？
6. 怎样保留并验证失败产物，而不是靠无限催办维持 busy？

这些机制有 Orca 的明确实现可参考，应成为我们的故障验收输入。是否直接移植模块、复用另一底座或只借鉴设计，需要下一阶段核对运行依赖与接口；本次不把源码阅读冒充已经完成了可移植性评估。
