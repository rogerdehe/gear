# The departure sheet (`gear --skill departure`)

Before every voyage departs, show the user its departure sheet in the chat and ask whether to depart. This applies to the first voyage and every voyage after it, including a short one. Show it when the voyage is fully decomposed: the config passes `gear admin init --check`, and the plan is written but not yet applied. The user reads it once, top to bottom, and says "depart" or writes what to change.

Rules:
- Write it in the user's language, as one Markdown table in the chat. Never use a dialog tool for it. GEAR's own words (task ids, commands, model ids, paths) stay as they are.
- Show it only when your own work is finished: every contract written, the loads balanced, every task in the plan. The sheet asks the user for decisions, not for patience.
- Every line comes from the config and the plan you are about to apply, the charters in `gear.toml` too. Show nothing you have not written, and leave nothing out that you will apply.
- It is not the plan. It aligns where the voyage goes, not how: one table, one row per item, never a row per task. Tasks and contracts stay in the plan.
- The sheet's title names the project: a bold line above the table, `**Departure: <project> · <voyage id> <title>**`, so the user knows which project is departing when several run from one pane. The table's header is only `| Item | Value |`.
- Keep every value cell within 60 columns (a CJK character counts as two, so about 30 characters), so the whole table fits the user's terminal: a table wider than the window is shown as cards, not as a table. Each done clause, station and lane gets its own row, and every row names its item: never a row with an empty first cell, never `<br>`. A cell that is too long is written shorter, not split.
- When the user changes something, change the config or the plan, then show the whole sheet again with each changed line marked `(changed)`. Repeat until the user says to depart.
- Start nothing before that word: no `gear admin init` without `--check`, and no `gear plan apply`. A user who has already ordered the voyage in their own words — its work and its staffing — has given that word: write the plan, depart, and show the sheet as the record of what departed. Then init, apply and open the board. The confirmed sheet is the user's consent for both.

## The form

The title line, then one table, nothing else:

```markdown
**Departure: <project> · <voyage id> <title>**

| Item | Value |
|---|---|
| Goal | <one short sentence> |
| Done 1 | <clause> |
| Done 2 | <clause> |
| Bar | <FASTEST, DELIVERABLE or STRICT> |
| Eta | <duration> |
| S1 | <what it delivers> |
| S2 | <what it delivers> |
| Tasks | <n> in all |
| Lane <name> | <cli> · <model id> · <its work>, <n> tasks |
| Reviewers | <n> × <cli> · <model id> |
| Charters | lanes: <one line>; reviewers: <one line> |
| On quota | <wait / fallback to <model>> |
| To decide | <only what the user must decide; or "nothing"> |

Depart?
```
