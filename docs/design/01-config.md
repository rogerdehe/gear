# 01 配置与项目登记

文件：`gear/config.py`、`gear/cli/admin.py`（init）。预算 100–130 行。

## 项目配置（用户写，`gear admin init` 登记，库里存版本）

```toml
[project]
id = "harbor"
herdr_session = "marina"          # 必填，显式绑定
repo = "/path/to/repo"

[quota]
lanes = 4
temps = 2
includes_captain = false

[captain]
pane = "w2:pM"
kind = "codex"
profile = "codex"

[[lanes]]
name = "heat"
pane = "w2:p3"
kind = "opencode"
profile = "opencode"
worktree = "/path/to/wt-heat"

[[temp_slots]]                     # 每个槽位 = 一个空 shell pane + 一个隔离 worktree
id = "s1"
pane = "w4:p1"
worktree = "/path/to/wt-temp1"

[[temp_slots]]
id = "s2"
pane = "w4:p2"
worktree = "/path/to/wt-temp2"

[timing]                           # 全部可省，用默认
tick_s = 4
cooldown_s = 90
sample_min = 20
review_timer_min = 45

[profiles.opencode]                # 可覆盖内置 profile（04）
busy_markers = ["esc interrupt"]
```

## 校验（init 时，失败整份拒绝）

- session、pane、worktree 存在（只读查询 Herdr / 文件系统）。新接入目标须是可用 shell pane；pane 里若仍有旧 Agent 在跑，先由用户停止并核对资源，init 不向运行中的 Agent 输入命令。重复 init 时已接入的座位只核对当前绑定。
- 每条线 kind 对应的 profile 已验证（04 `verified=true`），否则拒绝。
- 线数 ≤ quota.lanes；槽位数 ≥ quota.temps（不足则警告：同时运行的临时工实际受槽位数限制）；槽位 pane 当前是空 shell 且 cwd 等于其 worktree。
- 舰长与长期线：init 只登记座位，**由 GEAR 重新启动接入**（03），支持 resume 的 CLI 可带 `resume_ref` 保留原会话上下文。
- cooldown_s > tick_s。
- 同一规范化 worktree 不能被两个座位或槽位登记（写入 02 resources，跨项目唯一）。
- 显示**实际生效值**（含默认值来源）。
- 重复 init：只应用变化；不重复创建座位；改动已有座位的 pane/kind 视为换人，走 03 的重新登记（NEEDS_CHECK）。

## 不做

不自动发现 Agent、不推断队伍组合（初始化对话是最终形态功能）；缺失项报错让用户补，不按 pane 名猜。
