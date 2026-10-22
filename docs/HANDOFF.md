# GEAR 项目交接（2026-10-19）

用户已从 /Volumes/Data/myai/orch 完整复制项目到 /Users/roger/myai/gear（实际路径 /Volumes/Data/myai/gear），并在新目录启动 Agent。以后以 gear 目录为准。复制含 .git、全部文档和历史证据；尚无产品代码，未部署 GEAR。

## 先读

1. docs/PRD.md（v0.7，当前需求基线，38 个验收场景）。
2. docs/TECH-MVP.md（最小技术方案）。
3. docs/research/2026-10-18/plugin-feasibility.md（插件机制边界与四项探针）。
4. docs/research/2026-10-18/orca-lessons.md（Orca 的源码经验，不能只看 README）。

旧 decision.md 是历史候选，不是现行底座决定；prior-research.md 的 Firstmate/固定 Claude 主管推荐已撤回。validation.json 是此前对候选工具的实验记录，不是 GEAR 验收通过证明。

## 已确定，不要重新推翻或反复询问

- 项目名 GEAR：Goals Executed through Agent Relay。插件 ID herdr-gear，拟用 CLI gear，尚未安装该命令。
- 产品是 Herdr 插件，解决通用多 Agent 编排协作、任务停轮无人接续；不与 BMAD 强绑定。BMAD/Superpowers/其他 skill 均可选。
- 一个总负责人直接管理执行者，不加主管层。总控、执行者、审核者均可选择任何已验证适配的 Agent CLI，不固定品牌。
- 插件只增加一个常驻进程：看板、队列、持续催办、mailbox、SQLite 同进程。不可拆多个 daemon/UI/watchdog，不强制 systemd，不串接 Board/Dispatch。实际工作 Agent 和短命 CLI/Git/测试进程不属于额外常驻编排服务。
- 插件 tab 承载唯一 gear run；切 tab、隐藏视图、本地客户端断开不停止远端循环。真正关闭该 tab 会终止插件；下一次打开恢复。没有外部观察者时不可声称能自行重启/即时告警。此边界已写 PRD。
- SQLite 为插件自有，使用 HERDR_PLUGIN_STATE_DIR；不能直接改 Herdr 内部状态库。
- worktree 是 P0 基础：并行写入隔离，改派要交接写入权；会话回收与代码/分支清理分离。
- 初始化是对话：发现执行端可用 Agent → 提队伍方案 → 用户调长期线数量/类型组合、临时额度/用途 → 确认版本后落地。允许 Pi/Codex/OpenCode 等混搭；不按总控类型推断执行者。
- 用户主要通过本地 Herdr remote 连接远端 Herdr 操作 Agent。不要再说成要求用户另行 SSH 登录操作。执行端/session 必须明确绑定；UI 选中机器不自动改变 CLI 上下文。
- 总控用持久 mailbox 与结构化全局快照；10 条消息处理 1 条仍保留 9 条，逐项确认，崩溃可恢复。不靠聊天最后一条消息管理全局。
- Herdr Board 与 Dispatch 仅参考实现，不作运行依赖。Orca 必须借鉴已编码的故障防护：Task/Dispatch 分离、uncertain 不重复派发、消息 ack/代次、资源归属与安全回收。

## 下一步（尚未执行）

先做 plugin-feasibility.md 四项最小承载探针：单实例、同进程 UI/循环不互相阻塞、remote 断连继续、退出恢复不双开。然后按 TECH-MVP 做一个总控、两条异构长期线、一个临时审核名额的真实闭环。先证明停轮续做与消息不丢，再扩看板与十线。不把整个 PRD 一次性实现。

此交接本身只要求接收并确认，不等同用户授权立即改动其活跃 Agent、部署服务或启动整队实验。可完成文档阅读与就绪核对，后续按用户指示推进。

## 工具事实与证据

本机 herdr --version 为 0.9.1。插件 startup 是一次性钩子，不是 daemon 监督器；不存在通用 skill 自动安装字段。remote 插件在执行端安装，本地配置/快捷键不自动转发为远端动作。更高版本文档不能当成本机已具备的能力。

临时源码研究目录（不是项目运行依赖）：/tmp/orch-deep-research、/tmp/orch-orca-research、/tmp/orch-plugin-research。固定 SHA 与研究证据在 docs/research/2026-10-18。源码/测试阅读不等于实机测试。此前只实际操作过 File Viewer 本地打开 PRD、移动独立 tab。

用户偏好：短、确定、能落地；不接受未经核查就堆架构、不接受反复固定某 Agent 角色、不接受 BMAD 强绑定或一堆后台进程。完成已授权工作再报告，不停在计划里，也不假称已验证。
