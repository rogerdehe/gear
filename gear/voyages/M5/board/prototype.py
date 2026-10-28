#!/usr/bin/env python3
"""GEAR board prototype, M5 (fake data). Built on the M4 prototype (gear/voyages/M4/board/prototype.py):
same colours, symbols, views, keys and panels; README.md lists every change against M4.
Keys: 1-5 / Tab views · [ ] older / newer voyage · j/k or arrows select · Enter opens · q quit."""
import curses

NOW = "14:32"
# ---------------------------------------------------------------- fake data, one block per voyage
# M5 change 1: the board keeps every voyage; [ and ] pick one and the whole board shows that one only.
VOYS = [
    dict(
        V=dict(project="gear", milestone="M5", title="small tasks, the board, the goal rule", state="sailing",
               used_m=212, eta_m=480, projected_m=455, slow_lane="mast", bar="DELIVERABLE",
               goal="A small task lands without review in under 10 min; the board shows every voyage and uses "
                    "the full width; a departed goal changes only with the user's yes.",
               goal_changed=None),
        DONE=[(False, "small task under 10 min: now 14 min (T-262)"), (True, "the board shows every voyage"),
              (False, "goal lock: not started")],
        STATIONS=[("S1", "small-task procedure merged", "done", 180), ("S2", "board views to the prototype", "now", 330),
                  ("S3", "goal lock + v0.5.0", "next", 480)],
        LANES=["hull", "mast", "keel"],
        # id, lane, stage, title, est, actual, rounds, tokens k, cost $, deps, finished, marks
        T=[
            ("T-257", "hull", "done",      "small task: the plan marks it",            20, 18, 1,  530, .25, [],        "11:20", ""),
            ("T-258", "mast", "done",      "board: voyages and the [ ] keys",          40, 44, 1, 1080, .50, [],        "11:47", ""),
            ("T-259", "keel", "done",      "goal box, check and milestones",           30, 27, 1,  670, .31, [],        "11:35", ""),
            ("T-260", "hull", "done",      "small task: no review, accept proof",      30, 41, 2, 1400, .67, ["T-257"], "12:31", ""),
            ("T-261", "mast", "done",      "board: the full width",                    45, 52, 1, 1310, .60, ["T-258"], "12:57", ""),
            ("T-262", "keel", "review",    "goal lock: GOAL_LOCKED at apply",          40, 35, 1,  730, .31, [],        "",      ""),
            ("T-263", "hull", "rework",    "small task: the captain settles it",       25, 49, 2,  960, .44, ["T-260"], "",      "past its budget"),
            ("T-264", "mast", "working",   "board: Time & tokens by lane",             40, 22, 1,  380, .14, ["T-261"], "",      ""),
            ("T-268", "keel", "passed",    "doorbell names the station",               15, 12, 1,  300, .12, [],        "",      ""),
            ("T-265", "keel", "deps",      "goal-change line with the user's words",   35,  0, 0,    0, 0,   ["T-262"], "",      ""),
            ("T-266", "mast", "queued",    "board: graph draws dependencies",          60,  0, 0,    0, 0,   ["T-261"], "",      ""),
            ("T-267", "hull", "queued",    "release v0.5.0",                           15,  0, 0,    0, 0,   ["T-263", "T-266"], "", ""),
        ],
        EXC=[("hull", "T-263", "rework", "round 2, 49/25m: the captain scopes what is left"),
             ("mast", "-", "overloaded", "mast has 100m left, hull 15m: move T-266 to hull")],
        DECISIONS=[("11:05", "scope", "light procedure: a task marked small skips review", "user at kickoff", True),
                   ("12:40", "rework", "T-260 round 2: accept proof missing", "bug in normal use", False),
                   ("14:05", "contract", "T-262 v2: GOAL_LOCKED also covers milestone lines", "user agreed 13:58", True)],
        WAITING=["T-263: the captain's rework scope (hull is past its budget)", "T-262: review by review-3, 12 min in"],
        FRICTION=[("T-260", "hull", "unread", "accept proof needed a real plan file; wrote one by hand (8m)")],
        BURN=[0, 45, 160, 250],
    ),
    dict(
        V=dict(project="gear", milestone="M4.2", title="fixes from real use", state="landed",
               used_m=256, eta_m=360, projected_m=256, slow_lane="hull", bar="DELIVERABLE",
               goal="Every M4.2 task merged with no hand fix; init --check changes nothing; no run-dry item while "
                    "no lane can start work; released as v0.4.8.",
               goal_changed=None),
        DONE=[(False, "no hand fix: NOT met (3 hand fixes)"), (True, "init --check changes nothing (T-228)"),
              (True, "no run-dry item (T-232, in code)"), (True, "released as v0.4.8")],
        STATIONS=[("S1", "29 tasks merged", "done", 256), ("S2", "v0.4.8 released", "done", 360)],
        LANES=["hull", "mast"],
        T=[
            ("T-228", "mast", "done",  "init --check validates, starts nothing", 30, 41, 2, 1580, .73, [], "01:52", ""),
            ("T-241", "hull", "done",  "an urgent message mid tool call",        25, 110, 2, 3100, 1.42, [], "04:31", ""),
            ("T-248", "hull", "done",  "the doorbell says what to run",          10, 112, 3, 2540, 1.18, [], "05:32", ""),
            ("T-252", "mast", "done",  "branch templates in gear-model",         35, 48, 1,  910, .42, [], "04:36", ""),
            ("T-253", "hull", "done",  "the engine uses the templates",          30, 20, 2, 1920, .87, ["T-252"], "05:27", ""),
            ("T-256", "mast", "done",  "two projects need {project}",            25, 33, 2, 1480, .69, ["T-252"], "05:30", ""),
            ("T-182", "mast", "moved", "push race re-merge",                     20,  0, 0,    0, 0, [], "", "M5"),
        ],
        EXC=[], DECISIONS=[("18:15", "scope", "one folder per voyage under gear/voyages (user)", "", True),
                           ("19:00", "note", "voyage report: done (1) not met, 3 hand fixes", "", True)],
        WAITING=[], FRICTION=[("T-248", "hull", "read", "cargo fmt missing for the pinned toolchain")],
        BURN=[0, 60, 150, 230, 256],
    ),
    dict(
        V=dict(project="gear", milestone="M4.1", title="two fixes another captain needed", state="landed",
               used_m=75, eta_m=60, projected_m=75, slow_lane="keel", bar="FASTEST",
               goal="T-226 and T-227 merged and released as v0.4.7.", goal_changed=None),
        DONE=[(True, "both merged, v0.4.7 released")],
        STATIONS=[("S1", "v0.4.7 released", "done", 75)],
        LANES=["hull", "keel"],
        T=[("T-226", "hull", "done", "a restart takes the current config", 30, 18, 1, 520, .24, [], "00:22", ""),
           ("T-227", "keel", "done", "silent seats are reported",          30, 73, 2, 1600, .71, [], "01:04", "")],
        EXC=[], DECISIONS=[("14:05", "note", "voyage report: 75m against a 1h eta", "", False)],
        WAITING=[], FRICTION=[], BURN=[0, 40],
    ),
]

STAGE = {  # M5 change 7: one stage per task, in time order, in words
    "deps": "waits for {dep} to merge", "queued": "queued", "delivered": "delivered, not started",
    "working": "working", "asking": "waits for the captain's answer",
    "handed": "handed in, waits for a reviewer", "review": "in review",
    "verdict": "waits for the captain's verdict", "sentback": "sent back, rework queued",
    "rework": "reworking (round {round})", "passed": "passed, waits to merge", "merging": "merging (gate running)",
    "mergefail": "merge failed, waits for the captain", "done": "merged", "failed": "failed (rounds used up)",
    "moved": "moved to {to}",
}
OPEN = ("delivered", "working", "asking", "handed", "review", "verdict", "sentback", "rework", "passed", "merging",
        "mergefail")

C = {}
TABX = []
G = {}          # the picked voyage's data (M5 change 1)
TOP = [5]       # first row of a view, below the header
OPENED = {}     # lanes expanded in Time & tokens
DETAIL = [False]  # change 14: the header is folded unless d unfolds it
# minutes from departure when each task's work started and when it merged (the graph needs both)
WHEN = {"T-257": (5, 31), "T-258": (5, 58), "T-259": (8, 46), "T-260": (32, 102), "T-261": (59, 128),
        "T-262": (47, None), "T-263": (103, None), "T-264": (129, None), "T-268": (70, None),
        "T-228": (0, 52), "T-241": (30, 158), "T-248": (160, 254), "T-252": (60, 96), "T-253": (100, 215),
        "T-256": (150, 250), "T-226": (0, 22), "T-227": (0, 75)}



def pick(i):
    G.clear(); G.update(VOYS[i]); G["i"] = i


def colors():
    curses.start_color(); curses.use_default_colors()
    for i, (k, v) in enumerate(dict(ok=curses.COLOR_GREEN, warn=curses.COLOR_YELLOW, bad=curses.COLOR_RED,
                                    dim=8 if curses.COLORS > 8 else curses.COLOR_WHITE, acc=curses.COLOR_CYAN,
                                    mag=curses.COLOR_MAGENTA, blu=curses.COLOR_BLUE).items(), 1):
        curses.init_pair(i, v, -1); C[k] = curses.color_pair(i)
    C["b"] = curses.A_BOLD
LANE_COL = dict(hull="acc", deck="mag", keel="warn", mast="blu")
STAGE_COL = dict(done="ok", merging="acc", review="acc", working="warn", queued="dim",
                 deps="dim", delivered="dim", asking="warn", handed="acc", verdict="warn", sentback="bad",
                 rework="warn", passed="acc", mergefail="bad", failed="bad", moved="dim")
SYM = dict(done="●", merging="◍", review="◎", working="◉", queued="○", deps="○", delivered="◌", asking="◉",
           handed="◎", verdict="◎", sentback="↺", rework="↺", passed="◍", mergefail="✗", failed="✗", moved="→")


def put(w, y, x, s, a=0):
    h, wd = w.getmaxyx()
    if 0 <= y < h and 0 <= x < wd:
        try: w.addnstr(y, x, s, max(0, wd - x - 1), a)
        except curses.error: pass

def hl(w, y):
    """change 13: the selected row is lit across the whole width, in one colour."""
    h, wd = w.getmaxyx()
    if 0 <= y < h:
        try: w.chgat(y, 1, wd - 2, curses.A_REVERSE)
        except curses.error: pass

def bar(f, n):
    f = max(0.0, min(f, 1.0)); k = int(round(f * n)); return "█" * k + "░" * (n - k)

def hm(m): return f"{m//60}h{m%60:02d}m"

def words(t):
    return STAGE[t[2]].format(dep=", ".join(t[9]) or "-", round=t[6], to=t[11] or "-")

def wrap(text, width):
    out, line = [], ""
    for word in text.split():
        if len(line) + len(word) + 1 > width and line: out.append(line); line = word
        else: line = (line + " " + word).strip()
    return out + [line] if line else out


def header(w, view):
    V, T = G["V"], G["T"]
    wd = w.getmaxyx()[1]
    # change 1: line 0 lists every voyage, newest first; the picked one is lit, [ and ] move the pick
    put(w, 0, 1, f"GEAR · {V['project']}  ", C["b"])
    x = 4 + len(V["project"]) + 4
    for i, v in enumerate(VOYS):
        mark = "⛵" if v["V"]["state"] == "sailing" else "⚓"
        label = f" {v['V']['milestone']} {mark} "
        put(w, 0, x, label, (curses.A_REVERSE | C["b"]) if i == G["i"] else C["dim"])
        x += len(label) + 1
    put(w, 0, x + 1, "[ older   ] newer", C["dim"])
    put(w, 0, wd - 8, NOW, C["dim"])
    state = "⛵ sailing" if V["state"] == "sailing" else "⚓ landed"
    put(w, 1, 1, f"{V['milestone']}: {V['title']} · {state}", C["b"])
    # change 2: the goal, boxed and bright
    bw = wd - 3
    put(w, 2, 1, "╔═ GOAL " + "═" * (bw - 9) + "╗", C["warn"] | C["b"])
    y = 3
    for line in wrap(V["goal"], bw - 4):
        put(w, y, 1, "║ " + line.ljust(bw - 4) + " ║", C["warn"] | C["b"]); y += 1
    put(w, y, 1, "╚" + "═" * (bw - 2) + "╝", C["warn"] | C["b"]); y += 1
    if V["goal_changed"]:
        at, before, after = V["goal_changed"]
        put(w, y, 1, f"{'Goal changed':<14}{at}: {before} → {after} (the user agreed; Decisions shows both)", C["bad"] | C["b"]); y += 1
    # change 3: every row says what it is; the labels are titles (bold), not grey.
    # change 14: folded by default — one row each for the goal check, the milestone, time and tasks; d unfolds.
    L = C["b"]
    n = max(20, min(40 if not DETAIL[0] else 50, (wd - 60) // 2 if not DETAIL[0] else wd - 95))
    span_ = max(V["eta_m"], V["projected_m"], V["used_m"])
    cells = list(bar(V["used_m"] / span_, n))
    cells[min(n - 1, V["eta_m"] * n // span_)] = "│"
    if V["state"] == "sailing": cells[min(n - 1, V["projected_m"] * n // span_)] = "◆"
    if V["state"] == "sailing":
        diff = V["eta_m"] - V["projected_m"]
        short = f"expected {hm(V['projected_m'])} of {hm(V['eta_m'])}, " + (f"{hm(diff)} ahead" if diff >= 0 else f"{hm(-diff)} behind")
        when = (f"used {hm(V['used_m'])} · planned {hm(V['eta_m'])} · expected {hm(V['projected_m'])} "
                f"(slowest lane: {V['slow_lane']}), " + (f"{hm(diff)} ahead of plan" if diff >= 0 else f"{hm(-diff)} behind plan"))
    else:
        diff = V["eta_m"] - V["used_m"]
        short = f"took {hm(V['used_m'])} of {hm(V['eta_m'])}, " + (f"{hm(diff)} under" if diff >= 0 else f"{hm(-diff)} over")
        when = f"took {hm(V['used_m'])} · planned {hm(V['eta_m'])} · " + (f"{hm(diff)} under plan" if diff >= 0 else f"{hm(-diff)} over plan")
    tasks = [t for t in T if t[2] != "moved"]
    merged = sum(1 for t in tasks if t[2] == "done"); waiting = sum(1 for t in tasks if t[2] in ("queued", "deps"))
    going = len(tasks) - merged - waiting; moved = [t[0] for t in T if t[2] == "moved"]
    k = max(1, len(tasks)); a_, b_ = merged * n // k, going * n // k
    met = sum(d[0] for d in G["DONE"])
    now = next((st for st in G["STATIONS"] if st[2] == "now"), G["STATIONS"][-1])
    reached = sum(1 for st in G["STATIONS"] if st[2] == "done")

    def taskbar(x, y):
        put(w, y, x, "█" * a_, C["ok"]); put(w, y, x + a_, "▓" * b_, C["warn"]); put(w, y, x + a_ + b_, "░" * (n - a_ - b_), C["dim"])

    if not DETAIL[0]:
        first = next((s_ for ok, s_ in G["DONE"] if not ok), None)
        put(w, y, 1, "Goal check", L)
        put(w, y, 15, f"{met} of {len(G['DONE'])} met", C["ok"] if met == len(G["DONE"]) else C["bad"])
        if first: put(w, y, 30, "next: " + first, C["bad"])
        mx = max(90, wd // 2 + 10)
        put(w, y, mx, "Milestone", L)
        put(w, y, mx + 11, f"▶ {now[0]} {now[1]} ({reached} of {len(G['STATIONS'])} reached)" if now[2] == "now"
            else f"✔ all {len(G['STATIONS'])} reached", C["b"])
        y += 1
        put(w, y, 1, "Time", L); put(w, y, 15, "".join(cells), C["ok"] if diff >= 0 else C["bad"])
        put(w, y, 16 + n, short, C["ok"] if diff >= 0 else C["bad"])
        put(w, y, mx, "Tasks", L); taskbar(mx + 11, y)
        put(w, y, mx + 12 + n, f"{merged}/{len(tasks)} merged · {going} going · {waiting} waiting")
        y += 1
        put(w, y, 1, "Quality bar", L); put(w, y, 15, f"{V['bar']} — " + {"FASTEST": "only what breaks the chain or loses data blocks a task",
            "DELIVERABLE": "errors in normal use block a task; edge cases are logged",
            "STRICT": "severe issues block a task; medium ones are the captain's call"}[V["bar"]])
        put(w, y, mx, "d: unfold the goal check, milestones, time and tasks", C["dim"]); y += 1
    else:
        put(w, y, 1, "Goal check", L)
        for ok, s_ in G["DONE"]:
            put(w, y, 15, ("✔ " if ok else "✗ ") + s_, C["ok"] if ok else C["bad"]); y += 1
        put(w, y, 1, "Milestones", L)
        for sid, what, st, at in G["STATIONS"]:
            txt, a = {"done": (f"✔ {sid} {what} — reached at {hm(at)}", C["ok"]),
                      "now": (f"▶ {sid} {what} — now, planned by {hm(at)}", C["b"]),
                      "next": (f"· {sid} {what} — planned by {hm(at)}", C["dim"])}[st]
            put(w, y, 15, txt, a); y += 1
        put(w, y, 1, "Time", L)
        put(w, y, 15, "".join(cells), C["ok"] if diff >= 0 else C["bad"]); put(w, y, 17 + n, when, C["b"]); y += 1
        put(w, y, 15, "█ used   ◆ expected finish   │ planned end", C["dim"]); y += 1
        put(w, y, 1, "Tasks", L); taskbar(15, y)
        put(w, y, 17 + n, f"{merged} of {len(tasks)} merged · {going} in progress · {waiting} not started"
                          + (f" · moved out: {', '.join(moved)}" if moved else ""), C["b"]); y += 1
        put(w, y, 15, "█ merged   ▓ in progress   ░ not started", C["dim"]); y += 1
        put(w, y, 1, "Quality bar", L); put(w, y, 15, f"{V['bar']} — " + {"FASTEST": "only what breaks the chain or loses data blocks a task",
            "DELIVERABLE": "errors in normal use block a task; edge cases are logged",
            "STRICT": "severe issues block a task; medium ones are the captain's call"}[V["bar"]])
        put(w, y, wd - 20, "d: fold", C["dim"]); y += 1
    # change 15: the views are a tab bar that looks like one: framed tabs, the picked one lit, a rule under it
    y += 1
    x = 1; TABX.clear()
    put(w, y - 1, 1, "╭" + "─" * (wd - 4) + "╮", C["acc"])
    for i, t in enumerate(["1 Now", "2 Graph", "3 Workload", "4 Decisions", "5 Time & tokens"], 1):
        label = f"  {t}  "
        put(w, y, x, "│", C["acc"])
        put(w, y, x + 1, label, (curses.A_REVERSE | C["acc"] | C["b"]) if i == view else C["b"])
        TABX.append((x, x + len(label), i, y)); x += len(label) + 1
    put(w, y, x, "│", C["acc"]); put(w, y, wd - 2, "│", C["acc"])
    put(w, y + 1, 1, "╰" + "─" * (wd - 4) + "╯", C["acc"])
    TOP[0] = y + 3


def lane_now(lane):
    cur = [t for t in G["T"] if t[1] == lane and t[2] in OPEN]
    return cur[0] if cur else None

def idle_reason(n):
    T = G["T"]
    if G["V"]["state"] == "landed": return "retired at the landing"
    q = [t for t in T if t[1] == n and t[2] in ("queued", "deps")]
    if not q: return "nothing left planned for this lane"
    nxt = q[0]; by = {t[0]: t for t in T}
    wait = [by[d] for d in nxt[9] if d in by and by[d][2] != "done"]
    if wait: return f"next {nxt[0]} waits for {wait[0][0]} ({wait[0][1]}, {words(wait[0])})"
    return f"next {nxt[0]} is ready but not delivered"

def view_now(w, sel):
    T, LANES = G["T"], G["LANES"]
    wd = w.getmaxyx()[1]
    tw = max(32, min(60, wd // 3))          # change 5: columns follow the width
    y = TOP[0]; put(w, y, 1, "LANES  (Enter: what this lane did and has left)", C["b"]); y += 1
    for i, n in enumerate(LANES):
        t = lane_now(n); a = 0; sel_y = y if sel == i else None
        done = sum(1 for x in T if x[1] == n and x[2] == "done")
        left = sum(1 for x in T if x[1] == n and x[2] not in ("done", "moved"))
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{n:<5}", C[LANE_COL[n]] | C["b"] | a)
        if t:
            over = t[5] > t[4]
            put(w, y, 9, f"{t[0]} {t[3][:tw]:<{tw}}", a)
            put(w, y, 16 + tw, f"{bar(t[5]/t[4], 10)} {t[5]}/{t[4]}m", C["warn"] if over else 0)
            put(w, y, 34 + tw, words(t) + (f" · {t[11]}" if t[11] else ""), C[STAGE_COL[t[2]]])
        else:
            put(w, y, 9, "idle: " + idle_reason(n), (C["dim"] if G["V"]["state"] == "landed" else C["bad"]) | a)
        lm = sum(x[4] for x in T if x[1] == n and x[2] not in ("done", "moved"))
        y += 1
        tot = max(1, done + left); k = done * 12 // tot
        put(w, y, 9, "█" * k + "░" * (12 - k), C[LANE_COL[n]])
        put(w, y, 22, f"{done} of {done + left} tasks done · {left} left ({lm}m of estimates)", C["dim"])
        if sel_y is not None: hl(w, sel_y)
        y += 2
    y += 1; put(w, y, 1, f"NEEDS A LOOK ({len(G['EXC'])})", C["b"] | C["warn"]); y += 1
    for lane, tid, kind, why in G["EXC"]:
        put(w, y, 3, f"{tid} {lane:<5} {kind:<14}", C["warn"]); put(w, y, 29, why); y += 1
    y += 1; put(w, y, 1, "QUOTA", C["b"]); y += 1
    for p, win, u, r in [("tokenrhythm", "5h", 41, "22:00"), ("codex", "week", 63, "Oct 30")]:
        put(w, y, 3, f"{p:<13}{win:<6}{bar(u/100, 14)} {u:>3}% used  resets {r}",
            C["bad"] if u >= 90 else C["warn"] if u >= 60 else C["ok"]); y += 1
    return len(LANES)


def order_of(T):
    return ([t for t in T if t[2] in ("queued", "deps")] + [t for t in T if t[2] in OPEN] +
            sorted([t for t in T if t[2] == "done"], key=lambda t: t[10], reverse=True))

def view_graph(w, sel):
    """Dependencies drawn as a git graph (change 8): dev is the main line, newest on top; each task is a branch in
    its lane's colour that leaves dev where its work starts (after what it depends on merged) and joins dev at its
    merge ●; a task not started yet hangs dashed (┆) in its lane, naming what it waits for."""
    T, LANES = G["T"], G["LANES"]
    wd, h = w.getmaxyx()[1], w.getmaxyx()[0]
    col = {lane: 9 + 6 * i for i, lane in enumerate(LANES)}
    info_x = col[LANES[-1]] + 6
    tw = max(30, min(56, wd - info_x - 52))
    y = TOP[0]
    put(w, y, 2, "dev", C["ok"] | C["b"])
    for lane in LANES: put(w, y, col[lane] - 1, lane, C[LANE_COL[lane]] | C["b"])
    put(w, y, info_x, f"{'task':<7}{'what':<{tw}}  {'stage':<36}time", C["dim"]); y += 1
    events = []
    for t in T:
        s_, m_ = WHEN.get(t[0], (None, None))
        if s_ is not None: events.append((s_, 0, t))
        if m_ is not None: events.append((m_, 1, t))
    events.sort(key=lambda e: (e[0], e[1]), reverse=True)
    waiting = [t for t in T if t[0] not in WHEN and t[2] != "moved"]
    open_ids = {t[0] for t in T if t[0] in WHEN and WHEN[t[0]][1] is None}
    live = set(open_ids)
    rows = []

    def info(t):
        time_ = (f"{t[5]}/{t[4]}m" if t[5] else f"{t[4]}m") + (f" · {t[6]} rounds" if t[6] > 1 else "")
        return f"{t[0]:<7}{t[3][:tw]:<{tw}}  {words(t)[:36]:<36}{time_}"

    def verticals(skip=None):
        return [(col[o[1]], "│", C[LANE_COL[o[1]]]) for o in T if o[0] in live and o[0] != skip]

    for t in waiting:
        a = C[LANE_COL[t[1]]]
        rows.append((verticals() + [(col[t[1]], "○", a | C["b"])], t, info(t), C["dim"]))
        rows.append((verticals() + [(col[t[1]], "┆", a)], t, "", 0))
    for at, kind, t in events:
        c, a = col[t[1]], C[LANE_COL[t[1]]]
        if kind == 1:
            cells = verticals(skip=t[0]) + [(3, "●", C["ok"] | C["b"])] + [(x, "─", a) for x in range(4, c)] + [(c, "╯", a)]
            live.add(t[0])
            rows.append((cells, t, info(t) + f"  merged {t[10]}", 0))
        else:
            if t[0] in open_ids:
                rows.append((verticals(skip=t[0]) + [(c, SYM[t[2]], C[STAGE_COL[t[2]]] | C["b"])], t, info(t), C["b"]))
            cells = verticals(skip=t[0]) + [(3, "├", C["ok"])] + [(x, "─", a) for x in range(4, c)] + [(c, "╮", a)]
            live.discard(t[0])
            rows.append((cells, None, f"{'':<7}starts +{hm(at)}" + (f" after {', '.join(t[9])}" if t[9] else ""), C["dim"]))
    tasks_rows = [r for r in rows if r[1] is not None and r[2]]
    G["graph_rows"] = [r[1] for r in tasks_rows]
    seltask = tasks_rows[sel][1] if sel < len(tasks_rows) else None
    for cells, t, text, attr in rows:
        if y >= h - 3: break
        put(w, y, 3, "│", C["ok"])
        for x, ch, a in cells: put(w, y, x, ch, a)
        if text:
            put(w, y, info_x, text, attr)
            if t is not None and seltask is t and text.startswith(t[0]): hl(w, y)
            if t is not None and seltask and text.startswith(t[0]):
                if t[0] in seltask[9]: put(w, y, wd - 22, "◂ needed by " + seltask[0], C["warn"] | C["b"])
                elif seltask[0] in t[9]: put(w, y, wd - 22, "▸ waits for " + seltask[0], C["warn"] | C["b"])
        y += 1
    put(w, h - 2, 1, "● merged into dev   ◉ working   ◎ in review   ◍ passed / merging   ↺ reworking   ○┆ not started   "
                     "a branch leaves dev where its work starts and joins it where it merges · ↑↓ select: its deps light up", C["dim"])
    return len(tasks_rows)

def view_workload(w, sel):
    T, LANES = G["T"], G["LANES"]
    y = TOP[0]; put(w, y, 1, "WORK DONE AND LEFT, BY LANE  (minutes of estimate)", C["b"]); y += 1
    tot_d = tot_l = 0; ends = {}
    for i, n in enumerate(LANES):
        d = sum(t[4] for t in T if t[1] == n and t[2] == "done")
        l = sum(t[4] for t in T if t[1] == n and t[2] not in ("done", "moved"))
        nd = sum(1 for t in T if t[1] == n and t[2] == "done"); nl = sum(1 for t in T if t[1] == n and t[2] not in ("done", "moved"))
        tot_d += d; tot_l += l; ends[n] = G["V"]["used_m"] + l
        k = int(round(d / 8)); r = int(round(l / 8))
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{n:<5}", C[LANE_COL[n]] | C["b"])
        put(w, y, 9, "█" * k, C["ok"]); put(w, y, 9 + k, "░" * r, C["dim"])
        put(w, y, 40, f"done {nd} ({d}m) · left {nl} ({l}m)")
        if G["V"]["state"] == "sailing": put(w, y, 72, f"ends about {hm(ends[n])}", C["dim"])
        if sel == i: hl(w, y)
        y += 1
    y += 1; put(w, y, 3, f"all lanes: done {tot_d}m · left {tot_l}m · at the measured pace the last lane ends at {hm(G['V']['projected_m'])}", C["b"]); y += 1
    # change 9: say plainly whether the work is balanced
    if G["V"]["state"] == "sailing":
        first, last = min(ends, key=ends.get), max(ends, key=ends.get); gap = ends[last] - ends[first]
        put(w, y, 3, f"balanced: every lane ends within {hm(gap)}" if gap <= 30 else
            f"not balanced: {last} ends {hm(gap)} after {first} — move {last}'s next startable wave to {first}",
            C["ok"] if gap <= 30 else C["bad"])
    y += 2
    put(w, y, 1, "BURN-UP  (minutes of estimate merged, per voyage hour)", C["b"]); y += 1
    BURN = G["BURN"]; top = max(BURN + [tot_d + tot_l]); H = 8
    for row in range(H, 0, -1):
        lvl = top * row / H
        line = "".join(" ██ " if b >= lvl else "    " for b in BURN)
        put(w, y, 8, line, C["ok"]); y += 1
    put(w, y - H, 8 + 4 * len(BURN), f"  ─ total {tot_d + tot_l}m", C["dim"])
    put(w, y, 8, "".join(f" h{i:<2}" for i in range(len(BURN))), C["dim"])
    return len(LANES)


def view_decisions(w, sel):
    # change 10: a summary first; the full list (the M4 view) below it
    y = TOP[0]
    put(w, y, 1, "WHAT CHANGED THE COURSE  (★)", C["b"]); y += 1
    stars = [d for d in G["DECISIONS"] if d[4]]
    for t, k, what, why, star in stars or [("", "", "nothing", "", False)]:
        put(w, y, 3, f"{t} ★ {what}" if t else "nothing", C["warn"] if t else C["dim"]); y += 1
    y += 1; put(w, y, 1, "WAITING ON SOMEONE", C["b"]); y += 1
    for s in G["WAITING"] or ["nothing"]:
        put(w, y, 3, s, 0 if G["WAITING"] else C["dim"]); y += 1
    y += 1; put(w, y, 1, f"ALL DECISIONS ({len(G['DECISIONS'])})", C["b"]); y += 1
    for i, (t, k, what, why, star) in enumerate(G["DECISIONS"]):
        a = 0
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{t} {'★' if star else ' '} {k:<9}", (C["warn"] if star else 0)); put(w, y, 22, what)
        if sel == i: hl(w, y)
        y += 1
        if why: put(w, y, 22, "why: " + why, C["dim"]); y += 1
    y += 1; put(w, y, 1, "FRICTION REPORTS FROM AGENTS", C["b"]); y += 1
    for tid, lane, st, s in G["FRICTION"]:
        put(w, y, 3, f"{tid} {lane:<5}", C[LANE_COL[lane]]); put(w, y, 15, f"{st:<7}", C["warn"] if st == "unread" else C["dim"]); put(w, y, 23, s); y += 1
    return len(G["DECISIONS"])


def view_cost(w, sel):
    # change 11: grouped by lane (development and review apart); Enter on a lane shows its tasks (the M4 rows)
    T, LANES = G["T"], G["LANES"]
    y = TOP[0]
    put(w, y, 3, f"{'lane / task':<22}{'est':>6}{'act':>7}{'ratio':>7}{'rounds':>8}{'dev tokens':>12}{'dev $':>8}{'review tokens':>15}{'review $':>10}{'total $':>9}", C["dim"]); y += 1
    tot = [0, 0, 0, 0]
    for i, n in enumerate(LANES):
        mine = [t for t in T if t[1] == n and t[5]]
        dev_k = sum(int(t[7] * .75) for t in mine); rev_k = sum(t[7] - int(t[7] * .75) for t in mine)
        dev_c = sum(t[8] * .75 for t in mine); rev_c = sum(t[8] * .25 for t in mine)
        tot = [tot[0] + dev_k, tot[1] + dev_c, tot[2] + rev_k, tot[3] + rev_c]
        est = sum(t[4] for t in mine); act = sum(t[5] for t in mine); r = act / est if est else 0
        a = 0
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{'▾' if OPENED.get(n) else '▸'} {n:<19}", C[LANE_COL[n]] | C["b"] | a)
        put(w, y, 25, f"{est:>5}m{act:>6}m", a)
        put(w, y, 38, f"{r:>6.1f}x", C["bad"] if r > 1.5 else C["warn"] if r > 1 else C["ok"])
        put(w, y, 45, f"{sum(t[6] for t in mine):>8}{dev_k:>11}k{dev_c:>8.2f}{rev_k:>14}k{rev_c:>10.2f}{dev_c + rev_c:>9.2f}", C["b"])
        if sel == i: hl(w, y)
        y += 1
        if OPENED.get(n):
            for t in mine:
                r = t[5] / t[4]
                put(w, y, 5, f"{t[0]:<20}", C["dim"]); put(w, y, 25, f"{t[4]:>5}m{t[5]:>6}m", C["dim"])
                put(w, y, 38, f"{r:>6.1f}x", C["bad"] if r > 1.5 else C["warn"] if r > 1 else C["ok"])
                put(w, y, 45, f"{t[6]:>8}{int(t[7]*.75):>11}k{t[8]*.75:>8.2f}{t[7]-int(t[7]*.75):>14}k{t[8]*.25:>10.2f}{t[8]:>9.2f}", C["dim"]); y += 1
    y += 1; put(w, y, 3, f"this voyage: {(tot[0] + tot[2])/1000:.1f}M tokens (development {tot[0]/1000:.1f}M, review {tot[2]/1000:.1f}M), ${tot[1] + tot[3]:.2f}", C["b"])
    return len(LANES)


def lane_panel(w, lane):
    T = G["T"]
    rows = [(f"lane {lane} · tokenrhythm/deepseek-v4.1-flash", C[LANE_COL[lane]] | C["b"]), ("", 0), ("DONE", C["b"])]
    for t in [t for t in T if t[1] == lane and t[2] == "done"]:
        rows.append((f"  ● {t[0]} {t[3][:30]:<30} {t[5]}/{t[4]}m" + (f" {t[6]}r" if t[6] > 1 else ""), C["ok"]))
    rows += [("", 0), ("NOW", C["b"])]
    for t in [t for t in T if t[1] == lane and t[2] in OPEN]:
        rows.append((f"  {SYM[t[2]]} {t[0]} {t[3][:30]:<30} {words(t)}", C[STAGE_COL[t[2]]]))
    rows += [("", 0), ("LEFT, in order", C["b"])]
    left = [t for t in T if t[1] == lane and t[2] in ("queued", "deps")]
    for t in left:
        rows.append((f"  ○ {t[0]} {t[3][:30]:<30} {t[4]}m" + (f"  needs {','.join(t[9])}" if t[9] else ""), 0))
    rows += [("", 0), (f"left: {len(left)} tasks, {sum(t[4] for t in left)}m of estimates", C["dim"])]
    h, wd = w.getmaxyx(); bw = min(80, wd - 2); bh = len(rows) + 3
    win = curses.newwin(bh, bw, TOP[0], max(0, wd - bw - 1)); win.erase(); win.box()
    for i, (s, a) in enumerate(rows): put(win, i + 1, 2, s, a)
    put(win, bh - 1, 2, " any key: close ", C["dim"]); win.refresh(); win.getch()

def task_panel(w, t):
    T = G["T"]; by = {x[0]: x for x in T}
    rows = [(f"{t[0]} · lane {t[1]} · {words(t)}", C[LANE_COL[t[1]]] | C["b"]), (t[3], 0), ("", 0),
            (f"estimate {t[4]}m · actual {t[5]}m" + (" · over estimate" if t[5] > t[4] else ""), C["warn"] if t[5] > t[4] else 0),
            ("", 0), ("ROUNDS", C["b"])]
    if t[6] == 0: rows.append(("  not started", C["dim"]))
    else:
        rows.append((f"  dev 1      {t[1]:<6} {int(t[5]*0.7)}m   {int(t[7]*0.6)}k tokens", 0))
        if t[2] != "working" or t[6] > 1: rows.append(("  review 1   codex   7m   300k tokens" + ("  → fail: rework" if t[6] > 1 else "  → pass"), 0))
        if t[6] > 1:
            rows.append((f"  rework 2   {t[1]:<6} {int(t[5]*0.3)}m   {int(t[7]*0.2)}k tokens", 0))
            rows.append(("  review 2   codex   5m   180k tokens" + ("  → pass" if t[2] == "done" else "  (running)"), 0))
    rows += [("", 0), ("DEPENDS ON", C["b"])]
    rows += [(f"  {d} {by[d][1]:<5} {by[d][3][:30]}  ({words(by[d])})", C[STAGE_COL[by[d][2]]]) for d in t[9] if d in by] or [("  nothing", C["dim"])]
    rows += [("", 0), ("NEEDED BY", C["b"])]
    rows += [(f"  {x[0]} {x[1]:<5} {x[3][:30]}  ({words(x)})", C[STAGE_COL[x[2]]]) for x in T if t[0] in x[9]] or [("  nothing", C["dim"])]
    h, wd = w.getmaxyx(); bw = min(80, wd - 2); bh = len(rows) + 3
    win = curses.newwin(bh, bw, TOP[0], max(0, wd - bw - 1)); win.erase(); win.box()
    for i, (s_, a) in enumerate(rows): put(win, i + 1, 2, s_, a)
    put(win, bh - 1, 2, " any key: close ", C["dim"]); win.refresh(); win.getch()


def main(w):
    colors(); curses.curs_set(0); view, sel = 1, 0; pick(0)
    w.keypad(True)   # change 12: no mouse capture, so the terminal can select and copy text
    views = {1: view_now, 2: view_graph, 3: view_workload, 4: view_decisions, 5: view_cost}
    while True:
        w.erase(); header(w, view); n = views[view](w, sel)
        put(w, w.getmaxyx()[0] - 1, 1, "←→ / Tab / 1-5: views · [ ] older / newer voyage · d header details · ↑↓ select · Enter: a lane's work / a task's rounds / a lane's tasks · q quit      PROTOTYPE M5 · fake data", C["dim"])
        w.refresh(); k = w.getch()
        if k in (ord("q"), 27): break
        if k == ord("d"): DETAIL[0] = not DETAIL[0]
        elif k == ord("["): pick(min(len(VOYS) - 1, G["i"] + 1)); sel = 0
        elif k == ord("]"): pick(max(0, G["i"] - 1)); sel = 0
        elif k in (curses.KEY_RIGHT, ord("l"), 9): view, sel = view % 5 + 1, 0
        elif k in (curses.KEY_LEFT, ord("h"), curses.KEY_BTAB): view, sel = (view - 2) % 5 + 1, 0
        elif ord("1") <= k <= ord("5"): view, sel = k - ord("0"), 0
        elif k in (curses.KEY_DOWN, ord("j")): sel = min(sel + 1, n - 1)
        elif k in (curses.KEY_UP, ord("k")): sel = max(sel - 1, 0)
        elif k in (10, 13, curses.KEY_ENTER):
            if view in (1, 3): lane_panel(w, G["LANES"][sel])
            elif view == 2: task_panel(w, G["graph_rows"][sel])
            elif view == 5:
                lane = G["LANES"][sel]; OPENED[lane] = not OPENED.get(lane)

curses.wrapper(main)
