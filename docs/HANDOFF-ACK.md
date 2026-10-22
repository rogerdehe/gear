# GEAR 交接接收确认

接收日期：2026-10-19。当前项目目录：`/Volumes/Data/myai/gear`（`/Users/roger/myai/gear` 为同一路径），后续以 gear 目录为准。

已先读 `docs/HANDOFF.md`，随后读完 `docs/PRD.md`（v0.7，38 个验收场景）、`docs/TECH-MVP.md`、`docs/research/2026-10-18/plugin-feasibility.md` 和 `docs/research/2026-10-18/orca-lessons.md`。已阅读 Herdr skill，并确认当前环境 `HERDR_ENV=1`。

确认承接以下约束：

- 产品为 Herdr 插件 `herdr-gear`，拟用 CLI `gear`。唯一插件 tab 承载一个常驻 `gear run`，同进程负责看板、队列、催办、mailbox、SQLite 与请求入口；不增加独立 UI daemon、watchdog，不串接 Board/Dispatch 后台。实际 Agent 和短命命令不计入额外常驻编排进程。
- 一名总负责人直接管理执行者；总控、执行、审核均不固定 Agent CLI 品牌。无 BMAD 强绑定，研发 skill 可选。初始化通过版本化对话确认每条长期线的类型与配置、临时额度和用途规则，确认前不启动整队。
- 并行代码写入默认独立 worktree，改派须交接唯一写入权；会话释放、worktree 清理和分支删除分开，保留未交付成果及证据。
- 首版支持真实 Herdr remote：本地客户端操作目标执行端，进程、文件与持久状态位于执行端；明确绑定执行端/session，不以 UI 选中机器替代 CLI 路由，不另要求用户走一套 SSH 操作流程。
- SQLite 为插件自有，位于 `HERDR_PLUGIN_STATE_DIR`，不修改 Herdr 内部数据库。总控从结构化快照和持久 mailbox 恢复；十条事项处理一条后仍保留九条，逐项确认，支持崩溃恢复与代次隔离。
- 切 tab、隐藏视图、断开本地客户端不应停止远端循环；真正关闭唯一插件 tab 会终止编排进程，下次打开恢复。没有外部观察者时不承诺自行重启或即时告警。
- Board/Dispatch 仅作参考。Orca 借鉴 Task/Dispatch 分离、uncertain 先核对而不重复派发、持久消息 ack/代次、操作回执、资源归属与安全回收；源码及测试阅读不等于实机验证。

当前处于文档与设计基线阶段：尚无 GEAR 产品实现、部署或已通过的 GEAR 探针。旧 decision.md、已撤回的 Firstmate/固定 Claude 主管推荐和候选工具 validation.json 均不能替代现行基线或充当 GEAR 验收证据。

下一步在用户指示推进后，先做四项最小承载探针：单实例；同进程 UI/请求/循环互不阻塞；真实 remote 断连继续；退出恢复不双开，并核对禁用、卸载和升级边界。通过后再按 TECH-MVP 验证一个总控、两条异构长期线、一个按需临时审核名额的真实闭环，先证明停轮续做与消息不丢，再扩展完整看板和十条线。具体 Agent 组合须按初始化流程确认。

本轮仅完成交接阅读与本确认文件写入；未启动队伍、运行探针、部署服务或操作现有 Agent。
