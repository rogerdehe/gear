# Herdr 插件机制核查与最小落地边界

结论：插件能承载本产品的入口、终端看板、CLI 和自有 SQLite；Herdr 不提供 daemon 自动重启、任务持久化或 skill 自动安装。常驻控制循环必须由插件实现，但不必部署网络服务或强制使用 systemd。

**证据范围**

本机 `herdr --version` 为 0.9.1。已读取官方 v0.9.1 源码（commit `065ef9d6a531c49fb8bee7e818ef837065b21ee9`）、仓库保留的 0.9.1 文档、较新固定快照 `143bf5fd4781d164260c63fe28e7dc08054ad32f`，以及以下插件的固定版本。查看了测试断言，未运行这些上游测试，未安装探针插件，未连接/修改用户远端会话。此前本地 File Viewer 打开和移动 tab 已实际成功；不能把它算作本插件 remote/恢复验收。

**接口到底支持什么**

| 需求 | 核查结果与实现约束 |
|---|---|
| 插件入口 | manifest 声明 actions、panes、events、startup、build、link_handlers；commands 是 argv，插件自行选语言。不是加载到 Herdr 内部的业务模块 |
| 看板 | pane 可在 tab 打开，运行自己的终端程序；没有原生非终端 UI 扩展。看板必须自己写 |
| SQLite | Herdr 注入 `HERDR_PLUGIN_STATE_DIR`，插件自建数据库；没有 Herdr 托管存储 API。配置使用 CONFIG_DIR，不能写入安装 ROOT |
| 调用 Herdr | 使用注入的 HERDR_BIN_PATH 与 socket 上下文调用公开 CLI/API；runtime cwd 默认是插件目录，项目 cwd 从调用上下文解析并确认 |
| 后台常驻 | startup 是一次性初始化，不是监督器；链接/启用插件或客户端连接不会自动再执行 startup。action 成功启动不等于任务完成 |
| hooks | 可作快速提醒，不能替代持久消息；0.9.1 runtime 有最多 32 个并发插件命令限制，没有自动重试/持久投递承诺 |
| CLI 与 skill | manifest 没有通用 skill 安装和 CLI PATH 注册字段。可随包带文件，显式配置各 Agent；任务包提供准确 CLI 路径，不假定所有 Agent 自动发现 skill |
| remote | 插件及程序安装在目标执行端。插件 pane 在该端运行，客户端展示终端；数据库不跨网打开。本地自定义快捷键不自动转发为远端插件动作，应使用目标端入口/快捷键配置 |
| 生命周期 | 安装、禁用、卸载不等于后台的启动、停止。较新安装目录清理机制还要求关注存活后台引用旧 checkout；升级必须有显式停机/切换策略 |

**有没有现成插件这么干**

有“插件 + 常驻进程”的明确先例，不能把它说成 Herdr 自带的服务托管：

- **Herdr Board 0.19.1**：manifest 的 pane 运行 `board tui`，注释和 CLI 实现明确会自动启动 boardd。提供 daemon start/stop/status；operations 文档明确卸载不会停止后台。build 另行安装 CLI。它证明插件可以携带自己的常驻引擎，不证明 Herdr 会自动管理该引擎。
- **Herdr Dispatch 0.10.7**：startup 调 `scripts/start.sh`，脚本以 `nohup ... hdis daemon &` 启动后台，并提供 stop/restart actions。manifest 明说没有 shutdown hook，禁用/卸载后后台可能仍运行。
- **Herdr Tasks**：start.sh 同样通过 `nohup ... htask daemon &` 启动。不需要部署 Web 服务或 systemd。
- **File Viewer**：`[[panes]]` 运行长驻 TUI，提供 action 打开，是普通进程由 Herdr pane 承载的先例。它不是任务监督器。

本次检查的插件中，没有找到“专用 controller tab + 独立看板”完全相同的业务实现。这个方案是基于 Herdr 普通插件 pane 的设计选择，必须做生命周期实机探针，不能宣称已有现成插件验证了整个方案。也不直接照抄 nohup：其卸载遗留、旧版本存活以及较新目录清理约束都有成本。

**本项目的最小选择（按单进程约束修订）**

Board 与 Dispatch 只作为研发参考：分别学习任务/尝试与本地状态、启动/停止与去重；不把它们装成依赖，不拼接多个 daemon，不共享其数据库。上文描述的是这些插件自身实现，不是本项目选定架构。

产品为 Herdr 插件，在一个 tab 中运行一个 `gear run` 进程，内部合并看板、调度、自动催办、mailbox、SQLite 与 Unix socket 请求处理。CLI 请求及 Herdr/Git/测试命令可以短时运行；不新增第二个常驻 UI、服务或 watchdog。实际工作 Agent 不计作插件额外的常驻进程。

切走 tab、插件内隐藏视图或本地 remote 客户端断开不应停止远端循环。关闭唯一插件 tab 则会停止进程；没有独立后台可以继续。意外终止后下一次打开恢复，不假称它能自行重启。此前“独立 controller tab + board tab”的方案撤回。

**开始产品代码前的四项探针**

1. 在隔离 Herdr session 链接最小插件：action 打开唯一运行 tab；反复打开只聚焦同一进程，记录正确 cwd、socket、state dir。
2. 同一进程同时渲染表格、处理请求、定时写 SQLite；不操作键盘、停在详情页、隐藏视图均不阻塞计数。模拟慢调用，界面和其他请求仍能响应。
3. 目标端安装同一探针，经本地 Herdr remote 打开；切换 tab、断开重连后计数连续，所有文件留在目标端，进程清单只有一个插件常驻进程。
4. 终止插件 tab 后重开，记录可恢复且不双开；检查禁用/卸载和版本更换边界，不静默运行旧引擎、不删除数据库。先显式停止再卸载，不假定存在 shutdown hook。

这四项尚未执行，验证的是单进程插件承载机制；通过后再做真实 Agent 续做闭环。

**可复核来源**

- [官方插件文档（0.9.1 版本文档）](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/docs/versions/0.9.1/website/src/content/docs/plugins.mdx)
- [v0.9.1 manifest schema](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/api/schema/plugins.rs)、[命令与 startup 实现](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/app/api/plugins/runtime.rs)、[pane 与环境](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/app/api/plugins/panes.rs)
- [v0.9.1 remote 快捷键测试](https://github.com/herdrdev/herdr/blob/065ef9d6a531c49fb8bee7e818ef837065b21ee9/src/client/shell/tests/keybindings_settings.rs)
- [Board manifest](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/herdr-plugin.toml)、[Board 卸载与后台生命周期](https://github.com/nelsonPires5/herdr-board/blob/7a215a179acb4e098dd1c2971332497806309360/docs/operations.md)
- [Dispatch manifest](https://github.com/husniadil/herdr-dispatch/blob/852f94150a5159b261ca0e932aa126fb1f24b7ff/herdr-plugin.toml)、[Dispatch 启动脚本](https://github.com/husniadil/herdr-dispatch/blob/852f94150a5159b261ca0e932aa126fb1f24b7ff/scripts/start.sh)
- [Tasks 启动脚本](https://github.com/husniadil/herdr-tasks/blob/305c9ac978d3cdbafb07f15f031b9295a9a84be5/scripts/start.sh)
- [较新插件文档的安装目录与 detached 进程约束](https://github.com/herdrdev/herdr/blob/143bf5fd4781d164260c63fe28e7dc08054ad32f/docs/versions/0.9.5/website/src/content/docs/plugins.mdx)
