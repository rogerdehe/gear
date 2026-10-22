# 14 插件打包与运行

文件：`herdr-plugin.toml`、`bin/gear`、`cli/admin.py`（run）。预算 50–70 行。

## 清单

```toml
id = "herdr-gear"
name = "GEAR"
version = "0.1.0"
min_herdr_version = "0.9.1"
description = "Keep agent fleets working under one captain"
platforms = ["linux", "macos"]

[[panes]]
id = "engine"
title = "GEAR"
placement = "tab"
command = ["bin/gear", "run"]

[[actions]]
id = "open"
title = "Open GEAR"
contexts = ["workspace"]
command = ["herdr", "plugin", "pane", "open", "--plugin", "herdr-gear", "--entrypoint", "engine"]
```

- 无 `startup`、无 `events`（MVP）；不 nohup 后台进程。
- 包内带 `skills/gear/`（16）；安装说明包含 `npx skills add … --skill gear -g`。
- 引擎跑在插件 tab 里；重复打开时第二个实例拿不到锁，打印已有实例位置后退出（08）。
- `bin/gear`：`exec python3 -m gear "$@"`，并把插件根加入 `PYTHONPATH`。Agent 调用的 `gear` 通过安装时把 `bin/gear` 链接到 PATH（安装说明里一步），或任务包里写绝对路径。
- 状态：引擎用 `HERDR_PLUGIN_STATE_DIR`；被接入的 Agent 通过接入时设置的 `GEAR_DB` 找到同一数据库（03）。用户运维命令通过安装时记录的 `~/.config/gear/targets.toml`（执行端/session → 数据库路径）定位，并核对数据库 `meta.engine_session` 与当前 Herdr session 一致，不一致拒绝。MVP 每个执行端一个 Herdr session 绑定一个数据库，不按 UI 焦点猜。

## 生命周期边界

- 切 tab、隐藏、本地 remote 客户端断开：引擎继续。
- 关闭插件 tab：引擎停止；`gear send` 仍可入库；重开后恢复核对（08）。
- 升级：先停引擎（关 tab），替换代码，重开；schema 只升不降。

## 待探针

插件 pane 在 0.9.1 上的单实例与 remote 行为；`placement="tab"` 显示；Agent pane 中 `gear` 的 PATH。
