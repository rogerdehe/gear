# 11 复盘调度

文件：`gear/engine/review.py`。预算 90–120 行。判断为纯函数，提醒经 08 ops。
守住的不变量：复盘不靠舰长自觉；不无限催；提醒次数耗尽不等于完成。

## 五种触发（程序计算，不花 token）

| cause | 条件（初值，可配置） |
|---|---|
| `timer` | 距上次复盘结论 ≥ 45 分钟 |
| `milestone` | 里程碑内标 `required` 的任务全部 DONE（05 块头字段） |
| `low_ready` | 就绪任务数 < 可用长期线数，持续 ≥ 5 分钟 |
| `idle_lanes` | 有长期线 STANDBY ≥ 10 分钟 |
| `staff_change` | 有线进入 MISSING / RETIRING / RETIRED / NEEDS_CHECK |

## 合并与生命周期

```python
def evaluate(snapshot: ProjectSnapshot, open_reviews, now) -> list[ReviewChange]   # 纯函数；调用层先读好快照
```

- 同一 cause 同时只有一个未结事项（`V-n`，含已决定、等待 checkpoint 的）；条件持续期间只更新 detail，不新建。
- 舰长在草稿 `# Review` 写结论：`V-9 keep plan=<版本>: 理由; next=时间` 或 `V-9 change plan=<版本>: …; next=时间`（也可 `gear plan apply --keep V-9 --next <时间> "理由"`）。`plan apply` 校验结论关联当前计划版本与该事项，写入 `decided`、`checkpoint`；事项暂停。
- 到 `checkpoint` 时条件仍成立 → 事项重新到期；条件已消失 → 关闭。
- 结论只关闭它列出的复盘事项，不关闭任何消息。

## 怎么让舰长去做

- 舰长首页（13）顶部固定显示到期复盘及快照（各线状态、待命时长、就绪数、积压、最近产出、阻塞），**在消息批次之前**。
- 提醒：舰长空闲（观测确认一轮已正常结束）、有到期复盘、无占位中的投递（与门铃共用唯一投递位置）→ 投递一行"复盘到期（原因…），运行 gear"。和门铃共用每线单投递约束。
- 提醒最多 2 次（间隔 10 分钟）；仍无结论 → 看板标红 + `notify` 用户；事项保持到期状态，不标完成。
- 舰长正在工作时不打断；但复盘到期后，门铃内容改为"先处理复盘"，保证普通消息不会一直把它挤走。

## 测试要点

同因不重复建；持续更新；checkpoint 到期重开；消息持续到来时首页仍先显示复盘；两次提醒后通知用户且不关闭。
