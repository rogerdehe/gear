# GEAR trial feedback

Copy this file, fill it in, and send it back as one Markdown file. One file per trial; one block per problem. Write in any language. Exact output beats description: paste what the screen or the command printed.

## 1. About the trial

- Name / contact:
- Date:
- What you tried to build (one sentence):
- How far you got: install / kickoff / init / first plan / lanes working / a voyage landed

## 2. Environment

Paste the output of these commands:

```bash
gear --version
herdr --version
uname -sm
```

- Agent CLIs used, with versions and models (captain / lanes / reviewers):
- Project language and its build + test command (`gate`):

## 3. Problems

Copy this block once per problem.

---

### P<n>: <one-line title>

- **Type:** bug / confusing / missing / idea
- **Impact:** blocks me / I worked around it / minor
- **Where:** install / kickoff / init / plan / lanes / review / merge / board / quota / other
- **What I did** (the commands or what I asked the captain, in order):

  ```
  ```

- **What I expected:**
- **What happened** (exact output; a screenshot of the agent pane if it is about an agent):

  ```
  ```

- **Logs** (when the engine or a lane misbehaved):

  ```bash
  tail -50 ~/.local/state/gear/engine.log
  gear            # run in the captain's pane, paste the output
  ```

  ```
  ```

---

## 4. Overall

- What worked well:
- What was hardest to understand:
- Would you use it again for real work? Why / why not:
