# 13 命令面、首页、看板

文件：`gear/__main__.py`、`gear/cli/*.py`、`gear/engine/board.py`。预算 200–250 行。

## 日常命令（Agent 用）

| 命令 | 谁 | 参数 | 当场返回 |
|---|---|---|---|
| `gear` | 所有座位 | `--show <ID/线程/queue/draft>`（舰长 `--show draft` 导出新基准草稿到新文件）、`--diff`、`--json` | 角色首页（下） |
| `gear send` | 执行线、临时工、舰长 | `--kind progress\|question\|blocked\|result\|ack\|instruction`；`--task/--attempt/--contract`（result、blocked 必填；ack 按被回复事项校验，交接 ACK 不需要；任务包给出完整命令）；`--about T-x`、`--need`、`--to <座位>`（仅舰长）、`--reply-to M-x`、`--urgent`、`--candidate`、`--preempted T-x --saved <引用>`、`--request-id`；正文（参数或 stdin） | 07 的送达结果 |
| `gear reply` | 舰长 | `M-x …`、`--resolve`（缺省；只带 `--downgrade` 时缺省为 note）/`--wait <等谁> [--until <时间>]`/`--note`、`--downgrade`、`--next-check <时间>`、`--request-id`、正文 | 每个 ID 的处理结果 |
| `gear --skill [captain\|plan-format\|errors]` | 所有人 | — | 打印与当前版本匹配的 skill（16），只读 |
| `gear plan apply` | 舰长 | `[草稿路径]`（缺省项目 `plan.draft.md`）、`--keep V-n --next <时间> "理由"`（快捷：只交该复盘事项结论，关联当前计划版本）、`--request-id` | 05 的应用结果 |

执行线只需记 `gear` 和 `gear send`；任务包里给出可直接复制的完整命令。所有写命令在提交前先把 request_id 打到 stderr，丢回执时用它重跑。新舰长确认交接用 `gear send --kind ack --reply-to <交接事项>`，不另设命令。

## 运维命令（用户用，不给 Agent）

| 命令 | 作用 |
|---|---|
| `gear run` | 启动引擎（插件 pane 调用） |
| `gear admin init <project.toml>` | 登记/更新项目配置（01） |
| `gear admin seat add/confirm/remove …` | 登记现有 pane 为某条线；确认写入资源安全；移除 |
| `gear admin captain replace --project P --pane X --kind K` | 手动换舰长（03） |
| `gear --all` | 用户只读总览所有项目 |

运维命令需在用户自己的 shell 里执行（无 `GEAR_CTX`），并带 `GEAR_ADMIN=1` 或交互确认；Agent 调用一律拒绝。`seat add` 只登记座位，由引擎启动接入（03）。

## 首页内容

**执行线**：身份行（座位、代次）→ 当前任务一行 + 任务包路径 → 给我的未读回复/指示（原文）→ 合同更新通知 → 建议下一步命令。

**舰长**：
1. 交接/异常横幅（若有）；
2. **到期复盘**及快照（11）；
3. 各线一行：状态、当前任务、待命/停留时长、最近产出时间；临时工额度；
4. 收件箱批次（07）：线程、原文、ID；
5. 其他：未提及任务数、需核对事项、引擎心跳。

总长限制 ~120 行；超出写"另有 N，`gear --show …`"。读首页不改变任何消息/任务状态，只记录一条"查看记录"：座位、代次、时间，以及本次**实际展示**的任务/尝试/合同版本、消息 ID、关联操作 ID。输出成功后才写记录；输出与记录中断则视为未展示。该记录只证明内容已取到，不证明事项处理完或合同已采用。08 用它确认投递/门铃。

## 看板（插件 pane，`board.py`）

每轮刷新的文本：每个项目一块——舰长状态、各线状态与任务、临时工（在用/额度/排队/可恢复）、待处理数与最老等待、到期复盘、引擎心跳、观察失败。只读，不接受键盘操作（MVP）。不每轮更新 pane title（只在状态变化时更新）。

## 测试要点

各命令参数校验与错误码；首页只读；`--json` 结构稳定；执行线调用舰长命令被拒。
