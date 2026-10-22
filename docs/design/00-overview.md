# GEAR MVP 技术设计：总览

依据：[PRD v0.8](../PRD.md)、[TECH-MVP](../TECH-MVP.md)。本目录把 MVP 拆成模块骨架，逐个模块评审后再写代码。签名是设计约定，实现时可改名，但职责边界和合同不能偏。

## 1. 两个已确认的决定（2026-10-21，用户）

1. **临时工 resume**：机制上必须支持。**新活开新会话；同一件活中途需要它回来，找回原来那个临时工（resume 原会话）**。resume 不可用时退回新会话读任务包，并明确标为"新会话恢复"。见 [10-staff.md](10-staff.md)。
2. **换舰长只由用户手动切换**。MVP 不做任何自动切换或自动启用备用舰长；舰长不可用时只冻结、显示、通知。手动切换流程见 [03-identity-seats.md](03-identity-seats.md)。

## 2. 目录结构

```text
herdr-gear/                      # 插件根（herdr plugin link 指向这里）
  herdr-plugin.toml              # 插件清单：pane=引擎，action=打开看板
  bin/gear                       # 入口脚本：exec python3 -m gear "$@"
  gear/
    __main__.py                  # 子命令分发
    config.py                    # 01 项目配置加载与校验
    core/
      db.py                      # 02 SQLite、schema、事务、request 重放
      model.py                   # 02 枚举与数据类（状态机定义）
      identity.py                # 03 调用者定位、座位、代次、手动换舰长
    herdr/
      client.py                  # 04 Herdr CLI 封装
      profiles.py                # 04 各 CLI 的观测特征与 resume 模板
    plan/
      draft.py                   # 05 计划草稿格式解析
      apply.py                   # 05 校验与事务性应用
      render.py                  # 05 PLAN.md / 任务包 / 可编辑草稿生成
    work/
      tasks.py                   # 06 任务与执行尝试状态机
      evidence.py                # 12 证据、验收、产出采样
    mail/
      send.py                    # 07 发送合同与路由
      inbox.py                   # 07 线程选择与 reply
    engine/
      loop.py                    # 08 主循环与调度节拍
      observe.py                 # 08 Herdr 快照与座位对账
      ops.py                     # 08 副作用：意图→执行→结果
      dispatch.py                # 09 续做与派活（纯判断）
      doorbell.py                # 07 门铃
      staff.py                   # 10 待命/park/hold、临时工、退役/失联
      review.py                  # 11 复盘触发与提醒
      board.py                   # 13 插件 pane 文本看板
    cli/
      home.py                    # 13 gear
      send_cmd.py                # 13 gear send
      reply_cmd.py               # 13 gear reply
      plan_cmd.py                # 13 gear plan apply
      admin.py                   # 13 gear admin …（用户运维）、gear run
  skills/gear/                   # 16 Agent skill（SKILL.md + references），随版本发布
  templates/                     # 线说明、舰长说明（按项目生成）、催办/门铃/复盘提示
  tests/
```

## 3. 模块清单与预算

| 编号 | 模块 | 文件 | 产品行数预算 |
|---|---|---|---:|
| 01 | 配置与项目登记 | config.py、cli/admin.py（init 部分） | 100–130 |
| 02 | 数据库与事务 | core/db.py、core/model.py | 280–340 |
| 03 | 身份、座位、代次、手动换舰长 | core/identity.py | 170–210 |
| 04 | Herdr 适配 | herdr/client.py、herdr/profiles.py | 220–280 |
| 05 | 计划草稿、应用、视图 | plan/*.py | 270–330 |
| 06 | 任务与执行尝试 | work/tasks.py | 130–160 |
| 07 | 消息：发送、收件箱、门铃 | mail/*.py、engine/doorbell.py | 250–310 |
| 08 | 引擎循环、观测、副作用 | engine/loop.py、observe.py、ops.py | 220–280 |
| 09 | 续做与派活判断 | engine/dispatch.py | 150–190 |
| 10 | 人员：待命、临时工（含 resume）、退役、失联 | engine/staff.py | 260–320 |
| 11 | 复盘调度 | engine/review.py | 90–120 |
| 12 | 产出采样与验收 | work/evidence.py | 120–150 |
| 13 | 命令面、首页、看板 | cli/*.py、engine/board.py | 200–250 |
| 14 | 插件打包与运行 | herdr-plugin.toml、bin/gear、admin run | 50–70 |
| 16 | Agent skill | skills/gear/（Markdown，不计行数）；`gear --skill` 计入 13 | — |
| | **合计** | | **2,510–3,140** |

按第 15 轮审阅补齐身份固定上下文、交接、投递核对、resume 边界、验收闭环后，合计 2,510–3,140 行，**超过 3000 行预算**。这是设计估算，不是承诺。实现时先压展示、不压行为：合并零碎文件、简化看板与首页格式、计划视图格式化；不删同因合并、公平名额、提醒预算、单投递占位、resume 正常路径等已定行为。压缩后仍超出，带具体数字回到用户处决定。测试约 1,700–2,700 行另计，见 [15-tests-probes.md](15-tests-probes.md)。

## 4. 依赖方向

```text
cli/* ──▶ mail, plan, work, core, herdr(只读) （命令只做纯数据变化；Herdr 查询在写事务外）
engine/* ──▶ herdr, mail, plan, work, core （唯一执行 Herdr 副作用）
mail, plan, work ──▶ core
herdr ──▶ （仅 subprocess）
```

- `cli` 只能使用 `herdr` 的只读接口 `HerdrReadOnly`（身份指纹核对用 `agent get`），永不执行 Herdr 副作用。
- `engine/dispatch.py`、`review.py` 的判断函数是纯函数：输入快照与状态，输出建议动作；副作用统一经 `engine/ops.py`。

## 5. 公共约定

- **时间**：库内一律 UTC epoch 秒（int）；显示时转本地。
- **ID**：任务 `T-<n>`、消息 `M-<n>`、临时工 `R-<n>`、复盘事项 `V-<n>`、操作 `O-<n>`；按项目递增，不复用。
- **事务**：所有写操作经 `db.tx(conn)`（`BEGIN IMMEDIATE`，busy_timeout 5 秒）。事务内重新读取并核对身份、代次、版本。
- **request_id**：每个写命令自动生成或由调用方传入；同 ID 同参数返回原结果，同 ID 不同参数报错。
- **错误**：命令以非零退出码 + 一行 `ERROR <code>: <原因>` 输出；`<code>` 为固定枚举（见 02）。结果不明时退出码 3，输出 `UNCONFIRMED`。所有写命令在提交前先把 request_id 打到 stderr，供丢回执时重跑查回。
- **身份**：受管 Agent 都由 GEAR 启动接入，继承固定调用上下文 `GEAR_CTX`（03）；不按 pane 查最新代次。
- **输出**：默认人可读的短文本；`--json` 给结构化结果（Agent 也可用）。
- **不变量**：见 PRD §17，各模块文件标出自己负责守住哪几条。

## 6. 数据流（一个完整回合）

0. 用户 `gear admin init` 登记项目；引擎按配置启动并接入舰长与长期线（03）。
1. 舰长编辑 `plan.draft.md` → `gear plan apply`（05）→ 计划新版本入库，任务就绪，待执行的人员动作写入 operations。
2. 引擎循环（08）观测 Herdr → 续做判断（09）→ 给空闲线投递任务指针（ops）。
3. 执行线 `gear` 读任务包（13）→ 干活 → `gear send --kind progress/question/result`（07）。
4. 引擎发现舰长空闲且有待处理 → 门铃（07）。
5. 舰长 `gear` 看线程批次 → `gear reply` 逐项处理；需要改计划则回到 1。
6. 引擎定时：复盘触发（11）、产出采样（12）、临时工释放/恢复与待命到期（10）。

## 7. 评审顺序

建议按依赖从底往上：02 → 03 → 04 → 06 → 05 → 07 → 08 → 09 → 10 → 11 → 12 → 13 → 01 → 14 → 15。
