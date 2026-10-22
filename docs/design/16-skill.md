# 16 Agent skill

文件：`skills/gear/SKILL.md`、`skills/gear/references/*.md`（Markdown，不计代码预算）。`gear --skill` 打印、版本核对、采用记录与生成逻辑的代码计入 13、07、05、02 已有预算（不只 20 行）。

## 为什么要有

GEAR 的规则必须让 Agent 知道，但**不能靠催办消息广播**（旧 keeper 事故：改一句催办词就等于改了所有线的规矩）。照 Herdr 的做法，把"怎么在 GEAR 里工作"做成一个随版本发布的 skill：内容固定、有版本、和程序同步发布。

## 分工：skill、线说明、任务包

| 文件 | 内容 | 变化频率 |
|---|---|---|
| **skill**（所有项目通用） | 怎么用 `gear` 命令、汇报类型、ACK 规则、安全结束点、什么情况必须找舰长、禁止事项；舰长部分：怎么读收件箱、reply 模式、计划草稿格式、Verdicts、Staff、复盘结论、交接确认 | 随 GEAR 版本 |
| **线说明 / 舰长说明**（每个座位一份，引擎按项目配置生成） | 这条线的职责、worktree、要遵守的项目共享文档（如 INTERFACES.md）、授权边界 | 随项目配置版本 |
| **任务包**（每个任务一份） | 本任务合同、证据点、可直接复制的完整命令 | 随合同版本 |

通用操作说明集中在 skill，避免重复维护；线说明和任务包仍承载项目约束、任务合同和必要的完整命令。身份、代次、权限、状态迁移、安全条件仍由程序校验，不能因为规则写进了 skill 或 Agent 已 ACK 就省略检查。`GEAR_CTX` 存在只是阅读入口的提示，是否有效由 `gear` 核对。

## skill 结构

```text
skills/gear/
  SKILL.md                    # 入口：保护条件 + 角色判断 + 执行线全部内容（短）
  references/
    captain.md                # 舰长：收件箱、reply、计划草稿、Verdicts、Staff、复盘、交接
    plan-format.md            # 计划草稿严格格式（与 05 同源，生成而不是手写）
    errors.md                 # 错误码含义与该怎么做（与 02 同源）
```

SKILL.md 要点：

1. **保护条件**：没有 `GEAR_CTX` 环境变量 → 说明自己不是 GEAR 接入的 Agent，停止，不要运行写命令。
2. **先运行 `gear`**：首页会告诉你的角色（执行线/临时工/舰长）、当前任务和该做的事。舰长用 `gear --skill captain`，计划格式和错误说明分别用 `gear --skill plan-format`、`gear --skill errors`——不依赖安装目录里的相对路径。安装的 skill 只是发现入口，以当前运行版本的 `gear --skill` 输出为准；CLI 与引擎版本不一致时停止相关变更并提示。
3. **执行线必须遵守**：只用 `gear` 和 `gear send`；result/blocked 用任务包里给出的完整命令；收到需确认的事项用 `gear send --kind ack --reply-to <ID>`；小步保存成果；不创建脱离终端的后台写入作业；不修改计划和任务文件；遇到依赖用 `--about`，不要直接找别的 Agent；命令返回 `UNCONFIRMED` 时用打印出的 request_id 重跑，不要换个说法重发。
4. **舰长必须遵守**：在安全回合边界给到期复盘保留处理机会，普通消息不得连续挤走复盘；必要的紧急安全事项按既定规则优先；复盘可读取影响全局的消息，不逐条处理普通进展；不强行打断当前回合；只用 `gear reply` 关闭确切 ID；计划、人员、验收都通过草稿 + `gear plan apply`；不直接 prompt 其他 Agent。

## 发布与安装

- `gear --skill`：打印与当前版本匹配的 SKILL.md；`gear --skill captain` 打印 references/captain.md。
- 安装（有 skill 系统的 CLI）：`npx skills add <gear 仓库> --skill gear -g`，或复制 `skills/gear` 到该 CLI 的 skill 目录。
- **不依赖安装也能工作**：接入后的首条业务投递只发一次——在原有的任务派发（或交接包、或无任务座位的一次接入提示）中附上"先运行 `gear --skill` 读取本版本规则，再运行 `gear` 查看当前任务/交接事项"的入口指针和 skill 版本。不携带规则正文，不另发一条 skill 初始化投递。首投仍经 08 operations、遵守单待核对投递；读 skill 不代表任务包/交接包已展示，也不确认任务开始、合同采用或交接完成。resume 接入同样如此。
- **版本更新**：GEAR 升级后，复用 SYSTEM + needs_ack 消息，记录确切 skill 版本、收件座位与其会话代次；Agent 用 `gear send --kind ack --reply-to <更新事项 ID>` 确认（不需要任务参数）。事务内核对事项归属、代次、版本后保存采用记录，ACK 只关闭该事项。查看 skill、收到门铃、安装了某个副本都不等于采用；旧版本 ACK 不确认后来的新版本；旧会话的采用记录不算新代次已采用。
- **未确认新版前**：不往正在运行的回合硬塞规则，不静默改变在途合同或授权；不派发依赖新版规则的新工作；仍允许读取、提交更新 ACK、保存成果、报告阻塞与安全结束；无法安全继续的动作保持待核对。
- **待接班舰长**：读取交接指定的 skill 与 `gear --skill captain` 后，用既有交接 ACK 同时确认该交接引用的 skill 版本（交接事项保存版本引用）；确认时版本已变则要求核对当前内容。pending 阶段不另发独立的 skill 更新 ACK。
- 首页显示当前 skill 版本与该座位已采用的版本。

## 与其他模块的关系

- 13：增加 `gear --skill [captain|plan-format|errors]`（只读）。
- 14：插件包内带 `skills/` 目录。
- 05、02：`plan-format.md`、`errors.md` 由代码里的格式定义和错误码表生成，避免文档和代码两份不一致。
- 10/03：接入后的首条投递指向 skill。

## 不做

不给用户写单独的"安装引导 skill"（Herdr 的 agent-guide 那种），放到最终形态；MVP 只提供安装说明文档。
