# 参考项目与 Herdr 原生机制核查（2026-10-21）

源码均为固定快照只读研究，未在用户环境实机运行（除 2026-10-18 的测试记录）。完整证据：会话 scratchpad 中的 round9-codex-refs.md、round12-codex.md、round13-codex.md；源码目录与版本见 [../2026-10-18/sources.lock.json](../2026-10-18/sources.lock.json)。round9 补读的 tasks-axi@9401ff899c0d1d8ae6b4fd8727b9025abda2032c、no-mistakes@7212652ddf416eb385fb171d07140784f322f96c、herdr-kanban@b48a00f9deebac069398c5e8300ccea241c07d73 不属于 10-18 lock，版本和逐文件证据见 round9 第 12 节，不冒充 Firstmate 已安装依赖版本。

## 1. Herdr 0.9.1 插件机制（源码 /tmp/orch-plugin-research，文档 docs/versions/0.9.1）

- 官方定位："插件没有 SDK，整个 Herdr CLI 就是插件 API"；插件通过 `HERDR_BIN_PATH` 调 Herdr。
- manifest 可声明 `build`、`startup`（一次性，不是守护）、`actions`、`events`（事件钩子，每个事件起一个短命进程，全局并发上限 32）、`panes`（普通 Herdr pane，可承载常驻进程）、`link_handlers`。
- **plugin action 不能当 Agent 的业务接口**：调用参数只有 action_id / plugin_id / context，不能传正文（`api/schema/plugins.rs:306`）；命令在线程中启动后立即返回 Running 日志，输出截断 64KiB 只进插件日志（`app/api/plugins/runtime.rs`）。适合做用户快捷入口。
- 运行时注入 `HERDR_SOCKET_PATH`、`HERDR_BIN_PATH`、`HERDR_PLUGIN_STATE_DIR`、`HERDR_PLUGIN_CONFIG_DIR`、`HERDR_PLUGIN_CONTEXT_JSON` 及可用时的 workspace/tab/pane ID。
- 无插件存储 API；无可声明的持久定时任务。
- 事件：socket API 有 `events.subscribe/wait`；较新源码中可挂钩事件含 pane.agent_status_changed / pane.exited / pane.agent_detected 等，0.9.1 实际行为待探针。
- metadata token 键名限 `[A-Za-z0-9_-]{1,32}`，须带 source。
- `agent prompt` = 向 pane 终端写入文本（遵循 bracketed paste）+ 回车；不存消息。
- 命令数量（本机 `herdr <组>` 帮助列出的子命令路径）约 84，其中 agent 12、pane 28、plugin 11。

## 2. Firstmate（kunchenguid/firstmate @45dd50a）——最重要的同类参照

**是什么**：自称"Agent 发行包"。clone 下来的目录就是产品：`AGENTS.md`（433 行主控合同）+ 29 个 skill + 排除 tests/.git 后全仓库约 210 个 .sh 文件，另有 TypeScript（26 个文件，约 1 万行，Pi/omp 扩展、OpenCode 插件）、JS/MJS（约 5 千行）、Python（约 4.7 千行）。源码扩展合计约 14 万物理行（含空行、注释及内嵌文本，不是有效代码行或运行资源消耗）。在该目录启动受支持的 CLI，它就成为主控（first mate）。

**怎么运行**：没有统一服务。零 token 的 bash watcher 观察舰队、分类事件，只把需要行动的唤醒主控；watcher 如何一轮轮布防取决于主控 CLI（Claude Stop hook、Cursor stop hook、OpenCode 插件、Pi/omp 扩展、Codex 前台 checkpoint、Grok 后台调用）。host、away daemon 或扩展分支按 harness 和运行姿态选择，不是列出的所有后台必然同时启动；远端 secondmate 另有 job worker。核心状态以文件、目录和日志持久化；外接 backend/工具不能一概称无数据库。watcher 分类不调用模型，后续监督判断可能调用模型。

**主控支持**：Claude Code、Grok、Pi 并列首推；omp、Codex、OpenCode、Cursor 也已验证。监督深度随品牌不同：有人值守时只有 Claude/Cursor 由 supervision host 分担日常事件，Codex/OpenCode/omp/Grok 原样转交主控；Pi 用进程内分支。

**对照我们要的能力**：

| 需求 | Firstmate |
|---|---|
| 发消息当场知道结果 | 能：写入 inbox 后返回，门铃另报（远端/typed 路径有"未确认"结果） |
| 不绑 BMAD | 能 |
| 多项目 | 能：一个主控管多项目；各项目独立主控需多套 home |
| 临时工开关 | 部分：任务型 worker spawn/teardown；项目容量是可选的每项目上限，不区分长期/临时 |
| 停轮续做 | 部分：watcher 过滤并聚合；actionable wake 交主控或相应 supervision engine。Codex attended 的 host 为 pass-through，actionable wake 仍由主控处理，不代表所有原始事件都唤醒主控。 |
| 舰长不被淹 | 部分：持久记录、过滤、合并、分组 drain；尚未证明具备 GEAR 完整的线程前文与公平有界读序合同。 |
| 定期复盘 | 部分：heartbeat 600s 起、空闲退避至 7200s，无变化可吸收；收到 heartbeat 后要求全舰队复查并更新 backlog。未证明 GEAR 五类原因触发及持久结论闭环全部存在。 |
| 活计划插队 | 部分：backlog、依赖、hold；精确插队未证明 |
| 看产出/跑偏 | 部分：worktree 写入探测、验证与交付证据、主控核对；未证明具有通用自动语义跑偏检测。 |
| 长期线与轮空 | 部分：secondmate 持久且空闲健康，但它是可再派人的下级主控，不是平级执行线 |
| 依赖找谁 | 默认报给 firstmate 协调；有任务寻址发送能力，但未证明提供 worker 自治横向依赖发现与处理协议。 |
| 换舰长 | 人工换受支持 CLI 后基于磁盘恢复有基础；未证明通用额度耗尽自动跨品牌接管。 |
| resume | 无通用命令；部分后端有会话引用可参与恢复 |
| Herdr 插件、单进程、轻量 | 不能：Herdr 只是后端之一，多后台组件，大量依赖工具 |

**2026-10-18 本机测试**：多个关键套件失败或超时（watch-triage、wake-queue、checkpoint、dispatch-resolve、crew-state），见 [../2026-10-18/validation.json](../2026-10-18/validation.json)。不能据此说它不可靠，但本机未获得可靠性证据。当时未使用 live agents，Herdr 0.9.1 仅核对客户端版本；event-wait 的 5 项通过是 parser 单测，不是真实 Herdr/remote E2E。失败根因未全部建立，超时也不等于全套通过。

此前撤回推荐，主要因为为获得较完整监督路径而固定主控品牌、采用主管层的建议违背用户要求；Firstmate 本身并非只允许 Claude 或强制 secondmate。不同 harness 的监督行为不等价，填平差异涉及其监督架构；后来确定的单进程插件要求又构成部署不匹配。测试未闭环增加验证风险，但不是“它没有这些能力”的证据。GEAR 仍未证明自己更简单或更可靠。

**取舍**：不直接采用、不 fork；**重度借鉴**以下设计：

1. 发送"已持久存入"与"门铃结果"分开报告；丢回执按原关联 ID 核对，不重发（`bin/fm-send.sh`）。
2. watcher 先在程序里分类和合并（相邻事件 30 秒合并、良性事件吸收），只把需要行动的交给主控（`bin/fm-watch.sh`、`bin/fm-classify-lib.sh`）。
3. 有界 worktree 写入探测只作"不是卡死"的线索，无证据维持原节奏（`crew_worktree_written_since`）。
4. 声明式等待 `paused: ... until <时间>` 到点按原记录重评估——对应我们的 park。
5. 唤醒队列 drain 时分组呈现 OPEN DECISIONS / UNREAD STATUS 等（`bin/fm-wake-drain.sh`）——对应舰长首页。
6. worker 状态词汇：working / needs-decision / blocked / paused / done / failed，要求少报、只报有行动价值的变化（`bin/fm-brief.sh`）——对应 `gear send --kind`。
7. 回收前核对已落地成果和脏树，关闭失败保留待关闭记录；relaunch 用 journal 保留旧记录（`bin/fm-teardown.sh`、`docs/agent-control.md`）。
8. 会话锁 + 新会话读取持久摘要并对账（`bin/fm-session-start.sh`）——对应换舰长。

**不同之处（为什么它不是最终形态）**：我们要的是 Herdr 插件内单进程、主控不挑品牌且行为一致、平级长期线 + 临时工额度、活计划插队、引擎排复盘、线程化收件箱、轻量。

## 3. 其他参考

| 项目 | 消息/命令 | 借鉴 | 避开 |
|---|---|---|---|
| Orca | send=持久入队；ask 阻塞等答复；reply；Task/Dispatch 分离 | 结果不确定保留原尝试不重派；结算/留用/回收三分 | 批次 ack 当逐条处理；heartbeat 当续做 |
| herdr-tasks | 自带 CLI/MCP 调 daemon，同步返回；34 个动作 | 同一动作注册表；list 摘要/get 详情 | lease 到期或 pane gone 当可安全改派 |
| herdr-dispatch | 8 个动作；纯 Decide → 副作用 | 判断与副作用分离；项目轮转公平 | doing+idle 续做次数无上限；另起后台 |
| herdr-board | worker 只用 comment/done；57 条管理命令 | 执行者命令极少；done≠业务成功 | 停了没交结果只标 Awaiting 不续做；UI 与 daemon 分离 |
| herdr-workboard | CLI 直接改文件，同步调 Herdr | request_id 同参重放、异参拒绝 | 无上下文时猜焦点 workspace |
| herdr-kanban | 直接用 `agent prompt`，无收件箱 | 复用 Herdr metadata | 避免把按名称查找/复用主控直接当成 GEAR 的项目绑定与代次隔离；本轮未证明其具备我们的多项目多舰长合同 |
| bmad-loop | hook 回报事件；串行（max_parallel=1） | "停了却没交结果"的催办预算与活动证据 | hook 失败仍返回 0 |
| no-mistakes | Agent 用 8 个命令 + home | home 给当前状态和下一步命令 | 把可丢的事件缓冲当业务邮箱 |
| tasks-axi | 直接改存储并返回实际记录 | 修改后当场返回实际状态；ready 与 held 分离 | 日期 hold 当会启动模型 |
| /Users/roger/code/skills（BMAD-Team 等） | GitHub PR 评论当消息总线 | 消息格式（问题/选项/背景）、审核规则、优先级词汇 now/must/should/could/wont | 同一 story 多个问题被一条回复全部判为已答；批准不绑定版本 |

缺口：Jev、File Viewer、gh-axi 等外围项目没有固定源码，未审计。
