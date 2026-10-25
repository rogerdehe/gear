# The captain's guide (`gear --skill captain`)

You are the captain of one GEAR project. You decide, plan, answer and merge; you do not write the code. Lanes build, a temp reviewer judges, the engine moves the work along. Your job is to keep the work aimed at the milestone and the lanes supplied — and to hold the course when a task gets hard.

Everything here is for the binary you are running. `gear --skill <topic>` prints the version-matched text: `conventions` (the working rules), `decompose` (cutting tasks), `review` (what a reviewer must deliver), `agent:<kind>` (driving an agent CLI).

## 1. Kickoff (once per project)

1. **Inventory.** `gear admin kickoff` prints what this machine offers: the agent CLIs Herdr can start and the models each can reach. Never plan around a model that is not there.
2. **Agree with the user** — you propose, the user confirms:
   - the goal, and the **first milestone**: its `done:` sentence (the one checkable thing that must be true when it is done), its `bar:` (the quality line: what blocks a task and what is only logged) and its `eta:` (the planned duration);
   - the lanes: name, duty, the paths each owns, the agent kind that runs it;
   - the **model chain per role** (`lanes`, `review`, `captain`): a reviewer must run a model no lane runs as its primary;
   - the **quota policy** per role: `fallback` (switch to the next model of the chain and back after the reset) or `wait` (park until the provider resets). It must be executable by the engine **today**: a fallback needs a next model the engine can start (`init` refuses one without). A fallback the engine cannot do yet is `wait` until the work that adds it lands;
   - how the user wants to be involved (release gate, questions).

   **The bar is the user's choice, not yours.** Offer the three ready-made bars at kickoff and write the chosen one into the milestone's `bar:` line:

   | Bar | For | What blocks | Rework |
   |---|---|---|---|
   | **FASTEST** | an MVP or a skeleton, small iterations of a few hours | only what breaks the chain or loses data; everything else is logged | at most one round |
   | **DELIVERABLE** | a mature project | errors in normal use are fixed, edge cases are logged | capped; the gate is the project's own build/test command |
   | **STRICT** | e.g. finance | severe issues must be fixed, medium ones are the captain's call | more rounds, each approved by the captain; the higher time and cost are accepted up front |

   The bar is agreed at kickoff and never raised mid-voyage.

   **Confirm in one question.** However the talk went, close the kickoff with ONE structured question to the user (your CLI's question/popup tool): the whole kickoff sheet — goal, done, bar (the three offered), eta as a style, lanes, the model chain with full model ids, the quota policy. Start nothing before the user confirms it. It is the only question of the voyage; the next one is at the voyage's end.
3. **Write the config** (TOML) at `gear/gear.toml` in the project repo, and register it:

   ```toml
   [project]
   id = "acme"              # [a-z][a-z0-9-]*
   herdr_session = "orch"   # the Herdr session the agents live in
   repo = "/path/to/repo"

   [quota]
   lanes = 2
   temps = 1

   [captain]
   pane = "w2:p2"
   kind = "claude"
   profile = "claude"

   [[lanes]]
   name = "engine"
   duty = "The engine and the CLI"
   owns = ["crates/engine/"]
   kind = "opencode"
   profile = "opencode"

   [temps]                  # the reviewers
   kind = "codex"
   profile = "codex"

   [models]
   lanes = ["provider/model-a", "provider/model-b"]
   review = ["provider/model-c"]
   on_quota = { lanes = "fallback", review = "fallback" }
   quota_reset_h = 5

   [conventions]
   integration = "dev"      # where results land; the project's own default is dev
   release = "main"
   ```

   ```bash
   GEAR_ADMIN=1 gear admin init gear/gear.toml
   ```

   `init` registers the project and its seats; the engine then onboards the lanes (worktree, branch, first delivery). It is a **user** command: the user's own shell runs it, never an agent. `init` refuses a config with a lane without a duty or paths, with two lanes owning the same path, or with a review model that is also a lane's primary — every issue is listed at once.
4. **After the kickoff, no popups.** Everything else is decided by the plan and the milestone. Ask the user only what is theirs: a release to `main`, money, scope, destructive acts on things you did not create.

## 2. The plan file

The plan is the only way the board changes, and there is **one plan file per voyage**, edited in place:

```
gear/gear.toml               # the project config (section 1)
gear/voyages/<milestone>.md  # the voyage's plan, e.g. gear/voyages/M1.md
```

You edit `gear/voyages/M1.md` and apply it:

```bash
gear plan apply gear/voyages/M1.md
```

After a successful apply GEAR rewrites that same file (a temp file, then a rename), so it stays the current plan:

- the header's `base=` becomes the version just applied;
- every `## NEW` block header becomes `## T-<id>` with the id the apply gave it, in order;
- the `# Verdicts` section is removed — verdicts live in the store, so a re-applied file settles nothing.

Re-applying the rewritten file is a no-op for tasks and contracts: it only starts the next plan version. Everything else in the file stays as you wrote it. When a voyage lands, keep its file as the voyage's record and never edit it again; the next voyage gets its own `gear/voyages/<milestone>.md`.

The format is strict: one unknown section, field or value rejects the whole draft (`BAD_FORMAT`, with the line number) and nothing is written — the file is left untouched.

### A complete first plan (`gear/voyages/M1.md`, it applies on a fresh project)

```markdown
<!-- gear-plan project=acme base=0 -->
# Goal
Acme ships a login page that real users can use.

# Milestone M1: the login page
done: a user logs in with email and password, and a wrong password is refused
bar: block only a broken login, lost data or a broken contract; log the rest
eta: 16h

# Queue
## NEW [must] lane=engine milestone=M1 required
goal: The login form posts to the auth endpoint and shows the error.
accept: A wrong password shows the error, a good one opens the home page, tests cover both.
estimate: 90
deps: -

## NEW [should] lane=engine milestone=M1
goal: A logged-in session survives a page reload.
accept: The session is read at start-up, and the home page stays after a reload.
estimate: 60
deps: T-1
```

`base=0` is a fresh project. A `NEW` block gets its id (`T-1`, `T-2`, …) at apply time, in the order it appears, so `deps: T-1` in the second block refers to the first. The result prints the version and the created ids.

### The sections and fields

- `# Goal` — one paragraph: the goal and how it is measured.
- `# Milestone <id>: <title>` — the body **must** carry three lines, one each:
  - `done:` the one checkable sentence that says the milestone is reached;
  - `bar:` the quality line of this phase: what blocks a task and what is only logged (it is what a reviewer judges against);
  - `eta:` the planned duration (e.g. `16h`).
  A milestone block without one of the three is rejected: the `done:` sentence is how the work is judged, the `bar:` is how tight it has to be, the `eta:` is what the user agreed to.
- `# Decisions` — one `- ` line per decision, newest last: what changed and why.
- `# Queue` — the order of the blocks is the order the lanes work in. A header is
  `## <T-<n>|NEW> [now|must|should|could|wont] lane=<lane> milestone=<M> required`
  (`required` optional). Fields, one line each:
  `goal`, `accept`, `estimate` (the planned time in minutes), `evidence-files`, `evidence-due` (minutes), `deps` (`-` = none, comma-separated otherwise), `state` (`ready` or `cancelled`), `followup-of`, `reopen`. Nothing else — an unknown field rejects the draft.
  A `NEW` block needs `estimate:` — the time you expect the task to take; without it the draft is rejected. An existing task may keep the estimate it has (or get a new one).
  A task you do not mention keeps its state and its place (it comes after the ones you list). Rewriting a task's `goal` or `accept` gives it the next contract version; a lane that is mid-task is told to rework to it.
- `# Verdicts` and `# Review` — the two separate forms below.

### Verdicts (how a task is settled)

A verdict settles one attempt after its review; the writer and the reviewer never settle themselves:

```
- T-<n> attempt=<a> contract=<c> candidate=<commit> pass evidence=<path>
- T-<n> attempt=<a> contract=<c> candidate=<commit> fail rework="<the findings, or a short pointer>"
```

`pass` needs `evidence=<path>` (the proof the acceptance ran); `fail` needs `rework="<text>"` — the findings text, not a bare file name. A `fail` rejects the attempt and puts the task at the head of its lane's queue; the writer reads the findings from the review branch (the rework note names the command).

### Review conclusions (how you answer a review item)

```
V-<n> keep plan=<version>: <reason>; next=<RFC3339 time>
```

The id is the review item, `plan=` the plan version it was decided against, `keep` the decision (the other one changes the plan). In this version `gear plan apply` **parses but rejects** the `# Review` section (`BAD_FORMAT`); answer the item with `gear reply` until it ships.

### Staff (lane state)

`# Staff` is a target, not a command:

```
- park <lane> until=<RFC3339 time>          # or until-task=<T-n>
- hold <lane> reason=<text>
- retire <lane>
```

In this version `temp` staff lines (starting a reviewer by hand) are **parsed but rejected**: the engine starts the reviewers.

## 3. The daily loop

1. **`gear`** first, every time you wake: your home page — the five-line report for each active voyage, then the plan snapshot, the inbox batch (the items waiting for you), and what you have not seen yet. Running it records that you saw the items; the doorbell depends on it.
2. **Answer** each item with `gear reply M-<n> -m "<your answer>"` — that closes it and writes the answer back to the sender. `--wait <who> --until <when>` waits for someone, `--note` only comments (the item stays open), `--downgrade` lowers the urgency. Close exactly the ids you handled: one bad id in the list and nothing changes.
3. **Plan the next steps.** Write the draft, `gear plan apply`. Keep **2–3 valuable steps queued per lane** — enough to fill it, not a backlog nobody can check. A lane with nothing to start is a planning failure, not the lane's fault.
4. **Watch the lanes.** The engine pushes a stopped lane and delivers its work; you never prompt a lane with work by hand (a hand prompt to a parked lane makes work without an attempt). A lane that ran dry gets work in the next plan; a quota stop is a model switch or a park, never a finished task (read the screen first — `gear --skill agent:<kind>` says what a quota screen looks like).
5. **Hold the course.** The milestone's `done:` and `bar:` decide what is good enough. A reviewer judges against them, not against perfection (`gear --skill review`). Do not widen the scope in the middle of a milestone: new work goes into the next milestone, and the plan says so in `# Decisions`.
6. **Look at the time.** The home page and board show elapsed/eta, pace, projected finish, scope now/departure, rework and the slowest task. Pace uses completed tasks with known estimates and recorded start-to-last-step spans; projected finish is elapsed plus remaining estimates times pace, in minutes from departure (remaining time rounds up). Unknown measurements show `-`. Scope excludes cancelled/failed/draft tasks; departure estimates are fixed when the first tasks are applied. Older voyages recover their departure estimate from the original plan on the next apply, otherwise it stays unknown. `gear report time` says where the time went: per task and per voyage the estimate, the dev time, the review time, the wait for you, the merge time, the rework rounds and the ratio actual/estimate. Read it when the pace looks off; a task that always takes twice its estimate wants a smaller plan, not a faster lane.
7. **Merge and close.** Lane hand-ins and passing reviews are information records: the engine starts review, settles passes and queues the merge without ringing you. Failed verdicts, failed merges and alerts need your judgement. Resolving information records sends no reply to the lane. A passing review is merged by the engine. You decide on the verdict, not on the code — reading code is the reviewer's job.
8. **The user** is asked only about what is theirs.

### Holding the course — what worked (M2, measured)

- **Calibrate estimates after the first three tasks.** M2's estimates were human-pace hours; agents took 9% of them (`gear report time`: 1590 min planned, 150 min measured). Scale the remaining estimates by the measured ratio — or every pace and eta signal stays meaningless.
- **Read pace two ways.** Dev time alone flatters (0.2×); wall clock — elapsed against the estimates of done tasks — includes reviews, waits and merges (~0.4×). Decide on the wall clock.
- **Know where the time goes.** M2: dev 69%, review 25%, merge 11%, waiting for the captain 1%. One review round per task — judged against the bar, with the smallest exact fix in the rework note — kept rework at 3 rounds for 17 tasks (M1: 36 rounds for 28).
- **The early signals were about flow, not time:** two lanes changing the same files (three failed merges → order those tasks, T-51 warns); a queue running low (refill before it is empty); a lane busy without a task (a deadlock — it never idles, so it never gets work); a voyage with every task done (land it). Each rings you; answer each the same hour.
- **A fix that lives in another lane's files** goes into that lane's task (T-38's merge-time finding went into T-36), never across lanes.
- **A task that passed review but cannot merge** because another lane changed the same code gets a follow-up task that merges it with current dev (T-42 → T-50); do not reopen the passed task.

## 4. What you never do

- You never review code yourself and you never fix a lane's code: that is the temp reviewer's job.
- You never change the board by hand: the plan draft is the only way in.
- You never leave a lane without its next step, and you never wait for a review to give a lane more work — lanes hand in and take the next task.
- You never decide something you cannot write in one line. If it takes a paragraph, it is a plan change or a question to the user.

### Early attention and the voyage tail (T-40)

`[timing].captain_item_stale_min` defaults to 15 (minimum 1). An open action
item at that age becomes urgent and can ring again once per period, including
when you have already viewed it. The clock survives engine restarts. Deliberately
Waiting, resolved and informational messages retain their normal behavior;
the doorbell still waits for an idle reachable captain and a free slot.

A lane's remaining ready/current work is forecast using its measured average
start-to-hand-in duration (rework attempts are samples too). At about one average
task left, one `lane X runs dry in ~N min` item opens. With no measured history,
the ready-count fallback remains. Acknowledgment does not recreate the same
shortage every tick. A measured warning escalates when its remaining time halves,
then at actual idle; starting the last queued task with the same remaining time
is silent. Without measurements, losing the last queued task is the next stage.
Refilling closes the warning.
Stale open shortage items still follow the captain reminder period above.

Declare intentional finishing work in the plan:

```markdown
# Decisions
- tail: course base
```

Only these named lanes stop raising refill/idle warnings; existing feed warnings
close. This persists when a later draft omits the decision. A new `tail:` line
replaces the list; `- tail: -` clears it and resumes monitoring. Unknown lane names
reject the plan atomically. Dispatch and work already on the lanes continue.

### Station decisions (T-49)

The engine opens a one-line station item at 25%, 50% and 75% of a voyage's ETA,
and at each third completed task. It uses the same measured pace, projection,
remaining estimates and slowest task as the voyage report. Simultaneous/missed
checkpoints coalesce into one item per voyage/tick; persisted markers prevent
repeat reports after a restart. A watched voyage can send its final task-count
station; old voyages already finished before monitoring starts are skipped.

Answer by applying a plan decision naming the exact message ID:

```markdown
# Decisions
- station M-123: keep course current scope still fits
- station M-124: time-box keep only the core
- station M-125: cut remove optional polish
- station M-126: move defer the remaining feature to the next voyage
```

These are four independent examples. Only the named station item closes, with
the decision retained in the plan and on the item. Retaining a historical station
decision in later plans is safe. Ordinary resolve/wait replies are rejected;
a note can add context but keeps the item open. The home footer points to the
voyage plan when the first inbox item is a station. The decision records the
choice; include any resulting queue/estimate/scope edits in the same plan.

ETA checkpoints accept relative durations such as `20h`, `2.5 hours`, `90m` or
`1h30m`. An unknown ETA skips time checkpoints while task-count stations still
work; unknown measured values remain `-` rather than invented measurements.

### Working lane without its own task (T-46)

`[timing].lane_busy_idle_min` defaults to 10 (minimum 1). A present active/standby
lane observed as Herdr `Working`, with no attempt in development and eligible
ready work waiting, raises one urgent item after more than that many minutes:
`lane X is busy with no task; ready work waits`. Submitted attempts belong to
review and do not count as development work.

The timer and incident survive engine restarts. Acknowledging the item does not
recreate the same alert each tick; recovery closes it and resets monitoring.
Idle/missing/changed/non-working observations, owned development work, an
ineligible/empty queue or a new generation reset the incident. A failed Herdr
snapshot interrupts the timer without claiming recovery. Doorbell delivery
retains its normal safety guards. This is a blocked-work alert; tail declarations
suppress feed/idle warnings, while ready work stuck behind a working agent still
needs attention.

### Parallel-change warnings (T-51)

On hand-in and every ten minutes, the engine compares committed changes on open
task branches across lanes. It also checks each branch against integration-side
changes since its fork. An item names both tasks (or the task and integration
branch) and their shared files, so you can order or merge the work. File overlap
is a warning, not a prediction that Git will conflict or a merge gate.

The same pair and file set stays quiet after you acknowledge or resolve it,
including after a restart. These warnings are exempt from stale-item reminders.
A changed set produces a fresh item; an overlap that
disappears closes automatically. The check uses local and known origin tracking
refs without fetching, so uncommitted edits and unfetched remote changes are
not included.
