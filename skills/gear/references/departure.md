# The departure sheet (`gear --skill departure`)

Before every voyage departs, show the user its departure sheet in the chat and ask whether to depart. This applies to the first voyage and every voyage after it, including a short one. Show it when the voyage is fully decomposed: the config passes `gear admin init --check`, and the plan is written but not yet applied. The user reads it once, top to bottom, and says "depart" or writes what to change.

Rules:
- Write it in the user's language, as one Markdown table in the chat. Never use a dialog tool for it. GEAR's own words (task ids, commands, model ids, paths) stay as they are.
- Show it only when your own work is finished: every contract written, the loads balanced, every task in the plan. The sheet asks the user for decisions, not for patience.
- Every line comes from the config and the plan you are about to apply. Show nothing you have not written, and leave nothing out that you will apply.
- It is not the plan. It aligns where the voyage goes, not how: one table, one line per item, never a line per task. Tasks and contracts stay in the plan.
- The sheet's title names the project: start it `Departure: <project> · <voyage id> <title>`, so the user knows which project is departing when several run from one pane.
- When the user changes something, change the config or the plan, then show the whole sheet again with each changed line marked `(changed)`. Repeat until the user says to depart.
- Start nothing before that word: no `gear admin init` without `--check`, and no `gear plan apply`. Then init, apply and open the board. The confirmed sheet is the user's consent for both.

## The form

One table, one line per item, nothing else:

```markdown
| Departure: <project> · <voyage id> <title> | |
|---|---|
| Goal | <one sentence> |
| Done when | (1) <clause>; (2) <clause>; … |
| Bar | <FASTEST, DELIVERABLE or STRICT> |
| Eta | <duration> |
| Stations | S1 <what it delivers>; S2 …; (<n> tasks in all) |
| Lanes | <n> × <cli> / <model id> <args> |
| Reviewers | <n> × <cli> / <model id> |
| On quota | <wait / fallback to <model>> |
| To decide | <only what the user must decide; or "nothing"> |

Depart?
```
