# The departure sheet (`gear --skill departure`)

Before every voyage departs, show the user its departure sheet in the chat and ask whether to depart. This applies to the first voyage and every voyage after it, including a short one. Show it when the voyage is fully decomposed: the config passes `gear admin init --check`, and the plan is written but not yet applied. The user reads it once, top to bottom, and says "depart" or writes what to change.

Rules:
- Write it in the user's language, as Markdown tables in the chat. Never use a dialog tool for it. GEAR's own words (task ids, commands, model ids, paths) stay as they are.
- Show it only when your own work is finished: every contract written, the loads balanced, every task in the plan. The sheet asks the user for decisions, not for patience.
- Every line comes from the config and the plan you are about to apply. Show nothing you have not written, and leave nothing out that you will apply.
- It is not the plan. It aligns where the voyage goes, not how: one line per station and per seat, never a line per task. Tasks and contracts stay in the plan.
- When the user changes something, change the config or the plan, then show the whole sheet again with each changed line marked `(changed)`. Repeat until the user says to depart.
- Start nothing before that word: no `gear admin init` without `--check`, and no `gear plan apply`. Then init, apply and open the board. The confirmed sheet is the user's consent for both.

## The form

```markdown
## Departure sheet: <voyage id> <title>

**Goal:** <one sentence: what the user gets>

### 1. Done when
| # | Clause | Checked by |
|---|---|---|
| 1 | <clause of the done: sentence> | <the command, number or file that shows it> |

### 2. Bar and time
| | |
|---|---|
| Bar | <FASTEST, DELIVERABLE or STRICT>: <what blocks a task, what is only logged> |
| Eta | <duration>: <quick landing or do it properly> |
| Release | <version and when: at each station, or at the landing; or "none"> |

### 3. Stations
| Station | What the user gets | Tasks | Est. (min) |
|---|---|---|---|
| S1 <name> | <one line> | <n> | <n> |

Left out: <each backlog item not taken, and where it went; or "nothing">.

### 4. Staffing
| Seat | Agent / model (full id) | Repo | Owns | Load (min) |
|---|---|---|---|---|
| <lane> | <cli> / <model id> | <repo> | <paths> | <sum of its estimates> |
| reviewers × <n> | <cli> / <model id> | | | |

Quota: lanes <n>, temps <n>. On a quota limit: <fallback to <model> / wait>; agents through a long wait: <keep / release>.

### 5. Open points
<only what the user must decide, one per line; or "None." Your own unfinished work never goes here.>

**Depart now, or what should change?**
```

Fill every cell; write `-` where a cell has nothing. If the loads of the lanes differ by more than a third, rebalance the owned paths before you show the sheet.
