> 历史调研记录：其中固定 Codex 总负责人 / Claude 执行主管以及采用 Firstmate 为基础的推荐已撤回。当前结论见 [decision.md](decision.md)。此文件只保留当时源码观察与测试依据，不作为实施方案。

# Herdr 多 Agent 持续交付调研与选型

调研日期：2026-10-18（America/Chicago）。源码以 [sources.lock.json](sources.lock.json) 中的 commit 固定。本文是选型和实施设计，未把任何插件接入现有生产会话。

**决策：复用 Firstmate 的队伍管理、消息、监督与恢复机制，复用 Herdr 运行环境，复用 Captain’s Deck 终端看板；BMAD 保留需求、设计、story 与验收规范。保留 Codex 作为总负责人，先配一名 Claude Code 常驻执行主管，执行主管管理异构执行者与临时审核者。开发范围限定为 BMAD 衔接、精确队列操作和必要的兼容修复。不从零开发调度运行时，也不把 bmad-loop 放在整个并行队伍的总控位置。**

选型可以确定；生产可用性尚不能宣称。本机隔离测试暴露了失败和超时，必须作为上线门槛解决。源码测试证明的是机制，不证明某个真实模型永远不会停顿。最终应保证“未完成必有可恢复的记录、必有人或程序继续处理，失败必升级”，而非“每个任务必定无限自动成功”。

**1. 用什么标准选型**

用户的核心诉求不是必须保留 Bash 脚本，也不是必须一直使用同一个执行会话，而是：

1. 已授权任务不会因模型结束一轮而无人继续。
2. 主控不承担每个执行者的普通催办、重复状态问询和常规修复对话。
3. 需求能够拆成有验收条件的任务，形成可查看、可调整的队列。
4. 依赖出现后能调整派工，其他可开展的工作继续。
5. 原生 Codex、Claude Code、OpenCode、Grok、Pi 等可混用。
6. SSH 服务器上运行，终端中查看与控制；关掉看板不应停止任务。
7. 常驻管理职责与临时执行/审核会话有不同的生命周期。
8. BMAD 产物与研发规范持续保留，而不是迁移后丢失。

评价顺序：持续推进与恢复 > 主控负担 > 异构与并发 > 状态正确性 > BMAD 接入 > 队列交互 > 安装量。一个好看的看板不能弥补执行闭环缺失；一个支持无人值守的串行 story 引擎也不能自动变成十线调度器。

**2. 前两轮结论的纠正**

- BMAD Loop 已有外部 Herdr 后端：[pbean/bmad-loop-adapter-herdr](https://github.com/pbean/bmad-loop-adapter-herdr)。此前“未找到 Herdr 后端，因此需要自己适配”的判断不完整。
- Herdr Dispatch 确实处理已经认领且处于 doing 的 idle worker。不能只把它描述成“催未认领任务”。
- Firstmate 不是普通看板附件。它已有长期主管、异构 worker、持久收件箱、消息确认、监督、恢复、隔离工作树、回收与远程主管机制，应该是主要候选。
- 上一轮查看的网页缓存与当前仓库不完全同步。例如当前 bmad-loop 0.13.2 已含 BMAD v7 ticket-tree 模式。选型以本次固定源码为准，不混用搜索缓存的旧版本结论。

**3. 候选方案的实际边界**

| 方案 | 源码/文档确认的能力 | 对本需求的决定性不足 | 选型 |
|---|---|---|---|
| 原有四个脚本 | idle/done 续做、冷却、hold/mute、部分限流判断、汇报约定 | 没有统一任务事实、完成验收和持久事件确认；项目配置硬编码 | 作为故障案例和需求来源，不继续扩成总平台 |
| Herdr 本体 | 原生终端、Agent 状态、启动/提示/读取、工作区、插件入口 | 管进程与会话，不负责判定业务任务完成 | 保留运行底座 |
| hassox/herdr-kanban | pane 即卡片、列触发器、conductor | pane 生命周期与任务生命周期耦合，缺完整任务恢复闭环 | 不作为底座 |
| nelsonPires5/herdr-board | SQLite、TUI、卡片/运行历史、自动列流水线、异构启动、并发队列 | idle/done 未提交结果进入 awaiting；缺总控减负和长期主管层 | 不作为本次主底座 |
| husniadil/herdr-tasks + herdr-dispatch | 依赖就绪、任务租约、证据与审核、自动 worker、doing/idle 催办 | 两种可接收输入状态处理不一致；doing 催办无相应次数上限；provider 路径特化；审核/合入留给外部 | 不采用其总调度；参考任务租约和纯决策测试 |
| Phoobobo/herdr-workboard | 工作流与角色运行记录、TUI、幂等状态转移 | 无常驻执行引擎，不能独立解决停顿 | 不采用 |
| bmad-loop + Herdr adapter | 产物验证、修复重试、独立审核、恢复、停滞唤醒、BMAD v6/v7 接入 | 当前单个运行串行；并发设置被压成 1；外部 adapter 存在接口漂移；缺多队伍动态调度 | 保留为独立串行批处理选项，不叠加到默认队伍 |
| Firstmate + Captain’s Deck | 队伍/主管、异构 worker、持久消息、监督、恢复、回收、终端总览 | 工具链较多；Codex 主控监督能力有边界；精确队列编辑和 BMAD 双向衔接需补 | 采用其机制与核心，不宣称开箱完全满足 |

官方项目：[Herdr](https://github.com/herdrdev/herdr)、[Herdr Board](https://github.com/nelsonPires5/herdr-board)、[Herdr Tasks](https://github.com/husniadil/herdr-tasks)、[Herdr Dispatch](https://github.com/husniadil/herdr-dispatch)、[Firstmate](https://github.com/kunchenguid/firstmate)、[Captain’s Deck](https://github.com/deimantasnork/captains-deck)。

**4. 四项影响选型的源码证据**

**4.1 Herdr Board 能检测停止，但默认不恢复未完成任务。**

`crates/board-core/src/engine/signals.rs::decide_signal` 把 `Done` 和 `IdleExpired` 映射到 `CardStatus::Awaiting`，发出通知；`lifecycle.rs::decide_lifecycle` 对 Awaiting 的 timeout 返回 `TimeoutPaused`。这是明确的产品策略，并非漏看了某个 keep-alive 设置。结果是任务可能仍需外部主管处理。

[固定源码](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/crates/board-core/src/engine/signals.rs)

**4.2 Herdr Dispatch 的做了一半续跑机制仍有实际缺口。**

`internal/decide/decide.go::Decide` 只在 `task.Status == "doing" && agentStatus == "idle"` 时催办，`done` 没有同等分支。`internal/loop/loop.go` 将读到的状态原样写入快照，没有在这里把 done 归一化成 idle。doing 分支也没有使用 todo 分支的 `MaxPrompts` 上限。

本次把未修改的纯决策包复制到隔离目录，增加一个探针：任务 doing、100 次历史提示、配置上限 2。实际结果：idle 仍返回 Prompt；done、blocked、unknown、working 均不返回动作。测试文件与输出见 [探针](evidence/dispatch_probe_test.go)、[结果](evidence/dispatch-probe.log)。这不等于证明线上所有 done 都会永远停住，但足以否定“默认已正确覆盖 Herdr 所有正常停轮状态”。

[固定源码](https://github.com/husniadil/herdr-dispatch/blob/852f94150a5159b261ca0e932aa126fb1f24b7ff/internal/decide/decide.go)

**4.3 BMAD Loop 不是十线并行引擎。**

`src/bmad_loop/policy.py` 明确执行 `max_parallel=min(requested_parallel, 1)`。直接加载 `max_parallel = 10`，实测结果为 1；上游也有 `test_scm_max_parallel_clamped_to_one`。`docs/FEATURES.md` 把 Parallel units 标成 planned。

可以在隔离仓库里运行多个 loop，但那会额外引入任务唯一分配、story 状态合并、分支合入和跨运行依赖管理。那已经是在写并行总调度器，不能称为配置一下就有十线能力。更不能在同一个工作副本上开十个 loop，寄希望于状态文件写锁等同于任务唯一认领。

[固定源码](https://github.com/bmad-code-org/bmad-loop/blob/a5a8f09781e151eb5691f55d55316f47f9f16f1c/src/bmad_loop/policy.py)、[能力说明](https://github.com/bmad-code-org/bmad-loop/blob/a5a8f09781e151eb5691f55d55316f47f9f16f1c/docs/FEATURES.md)

**4.4 Firstmate 的监督并非所有主控 CLI 都相同。**

- Pi：同进程独立 supervision conversation，任务级常规处理可不占主对话。
- Claude/Cursor：supervision host 可在有人在场时处理常规事件。
- Codex：默认前台 checkpoint，期间不能推理；host 在 attended 状态是 pass-through。`quiet` 的 daemon 会过滤/批处理事件，但不能冒充具有同样判断能力的独立监督会话。
- Firstmate 的纯 Bash watcher 不消耗模型 token；判断如何修复、重排任务和重新派发仍可能消耗主管模型 token。不能宣传“整个编排零 token”。

因此本方案不赌原生 Codex 已有同等后台监督能力，而使用已有 secondmate 角色把十条执行线下沉到一名 Claude 主管。Codex 保留全局判断和用户沟通，不直接消费每个 worker 的普通停轮事件。

[监督 host](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/supervision-host.md)、[Codex 协议](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/supervision-protocols/codex.md)、[Pi 分支](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/pi-supervision-branch.md)

**5. 推荐的组织方式与控制权**

```text
用户
  ↕ 需求、关键决策、交付结果
Codex 总负责人（Firstmate primary）
  ├─ 持有 PRD/架构意图、任务拆解、优先级与变更权
  └─ 一名 Claude Code 执行主管（Firstmate persistent secondmate）
       ├─ 同一执行 home 的任务队列与监督记录
       ├─ Codex / Claude / OpenCode / Grok / Pi 临时执行者
       ├─ 独立审核和验证会话
       └─ Herdr panes + 隔离 worktrees

Captain’s Deck 显示各 home 的现有事实，不另存一份任务状态。
```

第一阶段只设一名执行主管，不机械地分十个组长。它的 charter 明确：在已授权任务与技术约束内持续推进；普通停轮、brief 已回答的问题、测试失败、断线恢复由自己处理；需求变化、接口语义冲突、预算耗尽、不可自行决定的事项才上报 Codex。

这增加一个长期模型会话，换来主控上下文隔离与批量汇报。它不是免费；应实测每完成一个任务的监督 token、总耗时与人工介入次数。小项目只有一两项短任务时可由主控直接监督，避免多一层的固定成本；这不改变十线项目的推荐拓扑。

Firstmate 已有 `data/secondmates.md`、charter、home seed、`config/secondmate-harness`、跨 home backlog handoff、路由回复和恢复流程。不需要自己实现“组长系统”。同机和服务器部署优先让整个 home 靠近代码；用户 SSH 到服务器使用 TUI。跨机器时采用整个 secondmate home 的远程路由，而不是让本地后台进程隔着 SSH 逐条轮询所有 worker。

**单一控制权必须严格保持：**总负责人定策略，执行 home 对其任务负责，worker 只能推进自己被分配的工作。总负责人调整正在执行的事项时，通过主管 steering 路径处理，不同时直接向该 worker 发第二套指令。主管接收更高层变更后更新 brief，并留下变更与执行确认。

重要限制：当前 Firstmate 将 `local-only` 项目留在 primary home，不能在不修改规则的情况下声称全部项目都可交给 secondmate。需要本地直合、不走 PR 的项目，应使用单 home 的 Claude 执行负责人加外部 Codex 策略输入，或把 local-only secondmate 支持作为明确扩展；默认落地选择 PR 交付，不偷偷改变用户原项目的交付方式。

**6. 一项任务具体怎样持续做下去**

以“实现 T-21，跑测试，提交审核”为例：

1. **准备**：任务记录包含 story 链接、输入版本、变更范围、验收命令、预期产物和依赖；依赖没满足不派发。
2. **派发**：Firstmate `fm-spawn.sh` 创建隔离 worktree 与 Herdr endpoint，并在启用 tasks-axi 时同步 In flight 状态。任务不是派出后才靠主控记住。
3. **普通停止**：worker 写两行后结束一轮。watcher 根据执行状态、任务声明、产物/活跃证据判断是否需要唤醒；可行动事件进入持久 wake queue。执行主管处理，不让用户先发现。
4. **续做**：主管通过 `fm-send.sh` 的持久 steering inbox 给出具体剩余工作。未确认的消息保留、重响，不能只凭一次 CLI 返回 0 当作任务已接收。
5. **纠偏与恢复**：先检查 brief 与当前产物；必要时定向纠正；确实卡死再通过 `fm-control.sh relaunch --note ...` 在原工作树替换会话。旧会话停止和新会话启动需核实，不能两个执行者同时写同一工作树。
6. **验证**：预配置测试/构建与独立审核验证产物；一个“完成”回复只是声明。分支、验证结果、审核结果与待交付项一致才可进入交付。
7. **失败处理**：重复修复无进展、重启也失败，保留成果与原因，形成阻塞/失败记录并上报。可开展的其他任务继续。
8. **回收与下一项**：交付条件满足后通过 `fm-teardown.sh` 回收自己拥有的会话/工作树；执行主管重新检查 ready 队列及容量，继续派发。

其中 2、4、5、8 的工具与数据通道已经存在；3 有 watcher 和 supervisor 协议；6 复用现有交付 gate；任务语义是否满足、何时换活仍需要主管判断。我们不声称所有策略已经是纯程序自动判定。

[恢复流程](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/.agents/skills/stuck-crewmate-recovery/SKILL.md)、[控制事务](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/agent-control.md)

**7. BMAD 怎么接，哪些东西直接复用**

BMAD 保持研发事实：PRD、架构、UX、epic、story/ticket、验收要求。Firstmate 保持执行事实：任务队列、执行归属、工作树、监督消息、审核/交付状态。

具体接入约定（这些是拟实现接口，不是冒充已有命令）：

- `orch bmad import <project>`：检测实际安装的 BMAD 代际和产物格式；按 project + story/ticket ID 幂等导入 tasks-axi；保留原文路径和内容摘要。一个 story 可拆为多个执行项，但父子映射必须记录。
- `orch bmad brief <task>`：生成给执行者的最小工作包：故事与验收条件、必要架构片段、接口契约、验证命令、禁止改动范围、结果格式。引用已安装的 BMAD 开发 skill，不把整份 PRD 广播给每个执行者。
- `orch bmad reconcile <project>`：由一个指定写入者把经过验证/交付的执行结果回写 BMAD 状态。任一必需子任务/审核未完成，父 story 不能被标 done。以实际版本决定 v6 YAML/story 或 v7 ticket 工具，不让多个 worker 直接改共享 sprint 文件。
- 任务启动时冻结需求版本；运行中需求变更必须留下修订记录和续做/重开决定，不能让同一个 task ID 静默换验收标准。

**交付路径只选一条。**默认复用 Firstmate 已支持的 no-mistakes 管理 review → test → docs/lint → PR/CI，BMAD 的验收条件与相关设计进入它的 review/test instructions。它支持异构 CLI、独立验证工作树和修复循环。

如果任务明确要求原生 BMAD 某个独立审核 workflow，则把该工作流作为具名独立审核项或验证 gate 接入并保存报告；不能谎称 no-mistakes 的普通 review 就等于那个 BMAD workflow。审核结论必须绑定同一候选 commit，审核后补丁需要重新验证。是否额外进行两套同范围 review，要在任务合同中明确，避免无意重复付费。

第一版不把 bmad-loop 和 no-mistakes 同时套在同一个 story 上。两套引擎都会运行验证、审核和状态推进，并可能各自创建 worktree/commit；这种堆叠使恢复与验收权不清。bmad-loop 可留给独立的串行批处理项目，不能与默认执行队列争用同一 story。

[no-mistakes 项目](https://github.com/kunchenguid/no-mistakes)、[可配置审核/验证入口](https://kunchenguid.github.io/no-mistakes/reference/repo-config/)

**8. 看板与精确队列：现成的和必须补的**

Captain’s Deck 已有按 crew/home 查看、工作/等待/完成视图、Agent 与模型信息、跳转 pane、Captain’s Call 决策输入及给 Firstmate 发消息。它使用 Firstmate snapshot，适合继续复用。

它目前不是完整的可编辑排程器。tasks-axi 有 ready 查询、依赖、priority、hold/unhold 与任务更新，但不能把 priority 0–4 宣传成任意精确排队，更不能说现成 TUI 已支持拖动任一 Agent 的任务队列。

需要补两个互相配合的表面：

| 补充能力 | 实现位置 | 状态归属 |
|---|---|---|
| 明确的工作线归属和精确队列顺序 | 扩展 tasks-axi task 模型/adapter，或经其受控更新保存等价字段；引入 lane 与 queue_rank | 仍由同一个任务后端写入 |
| 键盘移动、置顶、改派、暂停/恢复 | Captain’s Deck 的动作入口 | 调任务 CLI/Firstmate 命令，不直接改 snapshot |
| 卡片详情看到后续任务、依赖、验收、最新失败 | 扩展现有 snapshot projection | 只读投影 |
| 运行中改派 | 通过主管与 `fm-control` 事务，保留原任务与产物 | 先确认交接，再改变唯一 owner |

先支持键盘 `上移/下移/置顶/改派/暂停/恢复`，无需新 Web 应用。多个项目或 home 的跨队列移动复用已有 handoff；运行中任务不能当作一张静态卡直接移走。

如要任意 rank，必须同时修改排序与读取路径，并加并发写保护；只在 Markdown 正文写“排第一”不算实现。临时按 priority 调整可以作为过渡，但不是完整验收。

[Deck 控件与边界](https://github.com/deimantasnork/captains-deck/blob/058ff8af75b336ffdc8537b92c7e4825025be0f4/README.md)、[tasks-axi](https://github.com/kunchenguid/tasks-axi)

**9. 长期线和临时线的落地规则**

- 长期保留总负责人、执行主管、工作线责任与队列。只有确实需要持续项目上下文的职责才保留常驻会话。
- 具体实现任务默认短会话；相关连续任务可以保留上下文，但不以“永久养着十个聊天窗口”为默认。
- 独立审核使用新会话，只拿任务/设计/候选 diff/验证证据，不继承实现者对话。退出后保留报告与 commit 关联。
- 空队列不发送“还有活吗”消息，不消耗模型回合。新任务、依赖解除或明确复查时间触发下一次工作。
- 你原来的休息 20–30 分钟，主要是在规避脚本不断唤醒空线。采用事件与持久队列后，可直接安静等待；如果仍需要定时复查，另存 wake_at，不能使用永不自动恢复的 mute 标记冒充定时暂停。
- “临时 Agent 用完回收”指会话资源可释放；工作树必须满足产物保存和交付条件。一个进程退出不授权删除未提交工作。

**10. 依赖与动态调度**

现实项目不可能所有任务都无依赖。默认执行 ready 队列，而不是盲目执行文本列表的第一项。

例如 T-22 页面接入依赖 T-21 的接口契约：可把“定义契约”提前做成小任务，后续实现并行；若契约还不明确，T-22 阻塞，执行主管派其他 ready 任务。不要让执行者自行猜接口，也不要每次都暂停整个项目。

依赖之外，还要看共享外部资源、数据库迁移、同一环境部署、分支基线与最终合入。worktree 只隔离文件系统，不自动解决这些语义冲突。Firstmate 的 project capacity 是 worker 容量限制，不能冒充包括所有 gate 子进程在内的全局模型并发限制；部署时另核算审核/验证并发和资源开销。

**11. 要装多少东西，轻量性在哪里**

Firstmate 的维护成本不能隐瞒。它不是一个小 skill。当前文档列出的通用依赖包括 Node、Git、gh、no-mistakes、gh-axi、chrome-devtools-axi、tasks-axi、quota-axi；Herdr 后端还需要 jq、treehouse。Lavish 是呈现层可降级依赖，不要求为了本项目加 Web 看板。真实使用哪些功能与上游 bootstrap 要求哪些工具，是两个不同问题。

优点是这些都是已有工具与终端流程，不需要 Orca 的独立桌面工作环境，也不需要我们维护自己的 PTY、模型 SDK、Agent 启动器、远程控制协议和恢复数据库。

建议完整保留固定版本 Firstmate 核心；不要摘出几十个 shell 文件拼成一个所谓轻量版。这些脚本共享锁、消息确认、home 路由和权限约定，拆散后维护成本会回到我们自己身上。定制保持在私有 home 配置、BMAD bridge 与小范围 UI/任务模型补丁。

[依赖与配置](https://github.com/kunchenguid/firstmate/blob/45dd50a5e12bc3f5e009a4e18fa35ce7f7a71681/docs/configuration.md)

**12. 实施顺序与明确交付物**

| 阶段 | 实际工作 | 完成标志 |
|---|---|---|
| A 固定基础 | 固定 Herdr/Firstmate/Deck/依赖版本；隔离测试 home；修复或定位本次失败；保留旧脚本运行范围 | 与目标服务器版本配对的兼容记录，测试不触碰默认会话 |
| B 现成闭环 | Codex primary + 一名 Claude secondmate + 两种 worker；配置 brief、容量、汇报权限、恢复；打开 Deck | 人不催时，普通中断能恢复，主管失效能被发现，完成有证据 |
| C BMAD 衔接 | 幂等导入、最小任务包、版本检测、子任务映射、单写回、审核证据绑定 | 一个真实 story 从已批准需求到交付再回写，失败状态不会被抹掉 |
| D 队列交互 | lane/rank、排序、置顶、改派、暂停/恢复、详情投影 | TUI 修改影响实际下一次派发，关闭 TUI 不影响执行 |
| E 故障与规模 | 十任务压力、主管/worker 重启、断连、重复事件、依赖变更、审核失败 | 下列验收矩阵通过后再替换原脚本 |

这是集成与有限扩展，不是零开发。核心运行时不重写；BMAD 与排程交互需要工程工作。A 的测试问题未解决前，不应直接接管用户现有项目。

**13. 上线验收矩阵**

下列是拟定的交付门槛，不能当成本次已经全部实测通过。

| 注入场景 | 必须出现的行为 | 不允许出现 |
|---|---|---|
| worker 只改两行就停止 | 未通过验收；监督发现；自动续做或恢复 | 没人处理，或仅因 Agent done 标任务 done |
| idle 与 done 两种停轮状态 | 都进入任务状态核对 | done 因未归一化被漏过 |
| 一次提示提交响应丢失 | 先读当前状态/回执，再决定重送 | 重复执行同一派发 |
| working 但长时间没有任务进展 | 保守检查工具/验证进程；确认停滞后升级 | 单靠终端静默杀掉长测试 |
| supervisor 会话退出 | 持久记录仍在；父级发现并恢复执行主管 | 只能等用户看见 |
| daemon/watch 崩溃 | 未确认事件可重放；所有权不被重复取得 | 消息已丢却显示健康 |
| Herdr/SSH 暂时不可达 | unknown 与 dead 分开；连回后核对 | 误判死亡并双开 worker |
| T1 阻塞、T2 可做 | T1 记录具体依赖；T2 继续 | 全部工作停在队首 |
| 修改下一任务顺序 | 下次派发使用新顺序，历史有记录 | 只有看板顺序变了 |
| 临时 reviewer 完成 | 报告/commit 持久；会话可回收 | 复用实现者上下文冒充独立审核 |
| 队列空/人工暂停 | 无重复模型回合；新任务或定时条件可唤醒 | mute 永久遗忘、空线无限催办 |
| 十任务完成/失败混合 | 主管汇总；总负责人只看异常与决策 | 每个 token/每次普通修复都直接打扰总负责人 |

建议测量：人工“继续干”次数（目标 0）、无人处理的未完成任务数（目标 0）、重复派发数（目标 0）、停滞发现与恢复延迟、主控事件条数、监督 token、单位任务总时长。延迟门槛要按实际任务类型设置，不能对长构建和短文件修改一刀切。

**14. 本次验证的结果与局限**

本次克隆并固定 9 个仓库；检查关键源码及上游测试；使用临时工作树、模拟 CLI 与隔离状态进行验证。没有消耗真实 coding-agent 模型额度，没有调用用户现有 Agent，也没有升级 Herdr 服务端。本地客户端版本只读查询为 0.9.1，未据此推断服务端版本。

- BMAD Loop：11 个定向故障/策略用例通过，含未产生结果的停轮后续做、停滞唤醒、提示发送失败计数、重试、恢复、no-work 暂停、并发压制。见 [日志](evidence/bmad-targeted.log)。
- BMAD 较广测试运行：565 通过、1 失败后主动中止，以便聚焦关键场景；失败是 macOS 临时文件系统不接受测试构造的非法字节文件名，不能算实现逻辑缺陷，也不能算全套通过。见 [日志](evidence/bmad-tests.log)。
- Herdr 外部 adapter：111 通过、2 失败。一个是 `switch_client` 返回类型约定与当前核心漂移；另一个是 Windows 后端选择测试仍预期旧默认。这不能证明 Linux 启动必失败，但能证明最新版组合尚不能直接标为兼容。见 [日志](evidence/bmad-herdr-adapter.log)。
- Herdr Dispatch：纯决策 package 上游测试通过；研究探针确认 doing/done 与次数上限的实际行为。config package 也通过；loop package 在 10 分钟总时限超时，spawn 未取得完成结果。其余细节见验证汇总，不把未完成部分记为通过。
- Captain’s Deck：110 通过、2 失败；涉及 macOS `/var` 与 `/private/var` 路径比较，以及模拟启动 20 秒超时。将临时目录规范化为 `/private/tmp` 后单独复验这两项，2 项均通过（12.31 秒），没有改产品源码。不能因此把首次整套运行改写成全绿。
- Firstmate：Herdr event-wait 的 5 个 Python 用例通过；所选 6 个 shell suite 中出现锁/时序/状态断言失败及 300 秒超时。本机未获得这些 suite 全绿结果；有些 suite 在失败后仍继续输出，不能只凭退出码或末行判断成功。checkpoint 单独复验仍失败于 Perl fallback 的清理时序断言，不能仅归因于并行测试。原日志保留，需在目标 Linux 环境进一步区分产品问题、测试假设和机器负载。

这些结果足以确定复用边界和实现方向，不足以作生产认证。最终验证明细以 [validation.json](validation.json) 与 evidence 目录为准。

**15. 为什么这条路比继续扩脚本更合适**

它已经把你不断在脚本里补的共同问题做成可复用机制：消息先持久化再唤醒、确认后才消费、任务与会话分离、失败保留工作、主管自身被监督、暂停与异常分开、临时资源受控回收。我们需要补的主要是你的 BMAD 任务接口和排程体验。

继续扩原脚本，要重新解决这些基础问题；从 Herdr Board 开始，要另外补监督与恢复；从 bmad-loop 开始，要另外补并发调度。Firstmate 的代价是接受一套现成工具链与主管角色，但这是当前已发现候选中，距离“十个 Agent 持续工作、主控处理少量关键决策”最短的路径。

**最终采用路线：Firstmate 负责协作与持续监督，Herdr 负责运行，BMAD 负责研发合同，Captain’s Deck 负责终端可视化；只做连接层和队列交互扩展。先完成隔离兼容与故障验收，再迁移真实项目。**
