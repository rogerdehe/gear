# GEAR 开发台账（手动版，模拟 GEAR 的 PLAN 视图）

目标：GEAR MVP。当前里程碑：**M1 骨架 + 第一刀（消息闭环）**。
完成标准：`cargo test --workspace` 中 `gear-engine/tests/slice1_messaging.rs` 全部通过。

## 决定

- 2026-10-22 语言 Rust；由外向内切片；舰长搭骨架（model 全实现；store/herdr/app/engine/cli 签名 + 合同测试）；两条线实现。
- 2026-10-22 Herdr 适配直接走 socket，vendored herdr-board 的 board-herdr（MIT）。
- 2026-10-22 分支用 GitHub Flow：`main` + 每任务短命分支 `t<n>-<主题>`；每条线一个 worktree（`../gear-lane-a`、`../gear-lane-b`）；远端 `rogerdehe/gear`（私有）。

## 在途

| 任务 | 线 | 分支 | 内容 | 状态 |
|---|---|---|---|---|
| T-1 | A | t1-store | gear-store 实现 | 待派 |
| T-2 | B | t2-herdr | gear-herdr：profile、outcome、SocketHerdr | 待派 |

## 就绪（下一批）

| 任务 | 内容 | 依赖 |
|---|---|---|
| T-3 | gear-app：identity + mail（send/select_batch/reply）+ home | T-1 |
| T-4 | gear-engine：observe + doorbell + ops + tick，让 slice1 测试通过 | T-1、T-2、T-3 |
| T-5 | gear-cli：slice1 命令接线 + `gear run` 循环 + `--skill` | T-3、T-4 |
| T-6 | skills/gear/SKILL.md 第一版（舰长写） | — |

## 已完成

- M1-0 骨架（舰长）：workspace、gear-model（8 测试）、FakeHerdr（3 测试）、各层签名与合同测试。
