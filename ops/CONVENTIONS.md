# 开发约定（所有执行线必读）

## 你是谁、怎么工作

- 你是一条执行线，只做任务包里写的事。舰长（Claude）负责拆任务、定接口、审代码、合并。
- 每个任务一个任务包：`ops/tasks/T-<n>.md`。先完整读完再动手。
- 你在自己的 git worktree / 分支上工作，不要切换分支，不要动其他 worktree。

## 分支（GitHub Flow）

- 只有 `main` 一条长期分支，永远可编译、测试全过。
- 每个任务一个短命分支，命名 `t<n>-<主题>`（如 `t1-store`），由舰长从最新 `main` 切好放进你的 worktree。
- 舰长审完合回 `main` 并删除分支；下一个任务在同一 worktree 里从新 `main` 切新分支。
- 你不合并、不 rebase、不 push。

## 硬规则

1. **只改任务包"允许修改"里列出的文件。** 其他文件（尤其是 `crates/*/tests/*.rs` 合同测试、`gear-model`、各 crate 的 `lib.rs` 里已定义的公共签名）一律不改。觉得签名或测试有问题 → 写进报告，停下，不要自己改。
2. **不改公共 API**：不增删 `pub` 函数/类型/字段，不改函数签名。需要辅助函数就写私有的（`fn`，不加 `pub`）。
3. **不加新依赖**（Cargo.toml 不动），除非任务包明确允许。
4. **不运行会影响外部的命令**：不 `git push`，不操作 Herdr（不跑 `herdr agent prompt/start/...`），不删目录。
5. 小步提交：每完成一组能编译的改动就 `git commit`（信息写清做了什么），不要攒到最后。
6. 不创建后台常驻进程。

## 验收命令（任务包里会写具体包名）

```bash
cargo build -p <crate> 2>&1 | grep -E '^(warning|error)' && echo "有警告/错误" || echo "编译干净"
cargo test -p <crate>
```

要求：编译零警告零错误；任务包列出的测试全部通过。

## 完成或受阻时怎么汇报

写一个报告文件 `ops/reports/T-<n>.md`，然后结束本轮。**不要**给舰长发消息。报告格式：

```markdown
# T-<n> 报告
状态: done | blocked | question
提交: <最后一个 commit 的短哈希>
验收:
- cargo build -p …: 干净 / 有警告（贴出）
- cargo test -p …: N passed / M failed（贴出失败的名字）
做了什么: 3–5 行
问题/阻塞: 具体到文件和函数；需要舰长决定什么
```

`blocked` / `question` 时写清楚卡在哪、你认为需要什么，然后停下等待。
