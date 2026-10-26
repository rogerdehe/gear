#!/usr/bin/env python3
"""GEAR board prototype v2 (fake data). Keys: 1-5 views, j/k or arrows, Enter opens a lane, q quit."""
import curses

NOW = "14:32"
V = dict(project="gear", milestone="M4", title="one seat model, a voyage the user can review",
         used_m=192, eta_m=720, projected_m=655, slow_lane="mast")
DONE = [
    (True,  "reviewer seats kept and released; rework resumes its session"),
    (True,  "a lane rebinds to another repo; old tasks merge on their own"),
    (False, "a kind change runs in the same pane, target/ kept"),
    (False, "decision log + friction reviewed with the user at landing"),
    (False, "Herdr restart / no check-in: runs on with no hand fix"),
    (False, "v0.4.0 released"),
]
LANES = ["hull", "deck", "keel", "mast"]
MODEL = "MiniMax-M3.1-Flash"
# id, lane, stage, title, est, actual, rounds, tokens k, cost $, deps, finished
T = [
    ("T-135", "hull", "done",    "store: task keeps its repo",          40, 44, 1,  812, .31, [],        "11:02"),
    ("T-136", "deck", "done",    "profiles: session commands",          45, 50, 1,  900, .34, [],        "11:10"),
    ("T-137", "keel", "done",    "engine follows Herdr session",        40, 38, 1,  640, .24, [],        "11:21"),
    ("T-138", "mast", "done",    "plan hygiene (T-120)",                30, 33, 1,  520, .20, [],        "11:30"),
    ("T-139", "mast", "done",    "rebind a lane to another repo",       45, 71, 2, 1630, .62, ["T-135"], "12:44"),
    ("T-140", "hull", "done",    "kind change in place",                45, 52, 1,  950, .36, ["T-136"], "12:51"),
    ("T-141", "deck", "merging", "token count per round",               40, 39, 1,  702, .27, ["T-136"], ""),
    ("T-142", "hull", "review",  "auto check-in",                       30, 26, 2,  420, .16, [],        ""),
    ("T-143", "hull", "working", "one seat model",                      60, 38, 1,  610, .23, ["T-136"], ""),
    ("T-144", "deck", "working", "quota from provider APIs",            45, 22, 1,  330, .12, [],        ""),
    ("T-146", "keel", "done",    "overrun check at the estimate",       30, 41, 1,  880, .33, [],        "14:05"),
    ("T-147", "mast", "done",    "decision log in the voyage report",   45, 40, 1,  490, .19, [],        "14:20"),
    ("T-148", "hull", "queued",  "station projection per lane",         35,  0, 0,    0, 0,   [],        ""),
    ("T-149", "mast", "done",    "admin stop waits; gear whoami",       35, 30, 1,  400, .15, [],        "14:28"),
    ("T-150", "keel", "queued",  "friction reports at hand-in",         40,  0, 0,    0, 0,   ["T-142"], ""),
    ("T-151", "deck", "queued",  "board: graph view",                   50,  0, 0,    0, 0,   ["T-141"], ""),
    ("T-152", "deck", "queued",  "GitHub PR-mode landing",              60,  0, 0,    0, 0,   ["T-139"], ""),
    ("T-155", "deck", "queued",  "board: workload view",                45,  0, 0,    0, 0,   [],        ""),
    ("T-156", "deck", "queued",  "board: decisions view",               35,  0, 0,    0, 0,   [],        ""),
    ("T-153", "hull", "queued",  "reviewer release by demand",          30,  0, 0,    0, 0,   ["T-143"], ""),
    ("T-154", "deck", "queued",  "small fixes from M3.1",               40,  0, 0,    0, 0,   [],        ""),
]
EXC = [("hull", "T-142", "rework",        "round 2 in review: check-in must not fire on a rebase"),
       ("deck", "-",     "overloaded",    "deck has 7 tasks (315m) left, mast none, keel 40m: move T-152, T-155 to mast")]
QUOTA = [("MiniMax", "5h", 64, "15:12"), ("MiniMax", "week", 31, "Nov 02"), ("Codex", "week", 12, "Nov 02")]
DECISIONS = [
    ("11:40", "scope",    "PR-mode landing added as T-152 (user at kickoff)", "", False),
    ("12:05", "cut",      "light procedure moved to M5", "needs token data from T-141 first", True),
    ("12:40", "override", "T-139 F2 (log wording) logged, not reworked", "edge case under DELIVERABLE", True),
    ("13:10", "rework",   "T-139 round 2: rebind waits for the open merge", "bug in normal use", False),
    ("14:20", "split",    "T-146 split: restart hook goes to T-155", "lane stuck on an unclear contract", True),
]
FRICTION = [("T-139", "mast", "unread", "two-repo fixture had no gate script; wrote one by hand (10m)"),
            ("T-140", "hull", "read",   "scripts/test.sh ran 4 min under 4 parallel builds")]
# done minutes of work per hour of the voyage (burn-up)
BURN = [0, 0, 95, 170, 260, 330]

C = {}
TABX = []
def colors():
    curses.start_color(); curses.use_default_colors()
    for i, (k, v) in enumerate(dict(ok=curses.COLOR_GREEN, warn=curses.COLOR_YELLOW, bad=curses.COLOR_RED,
                                    dim=8 if curses.COLORS > 8 else curses.COLOR_WHITE, acc=curses.COLOR_CYAN,
                                    mag=curses.COLOR_MAGENTA, blu=curses.COLOR_BLUE).items(), 1):
        curses.init_pair(i, v, -1); C[k] = curses.color_pair(i)
    C["b"] = curses.A_BOLD
LANE_COL = dict(hull="acc", deck="mag", keel="warn", mast="blu")
STAGE_COL = dict(done="ok", merging="acc", review="acc", working="warn", queued="dim")

def put(w, y, x, s, a=0):
    h, wd = w.getmaxyx()
    if 0 <= y < h and 0 <= x < wd:
        try: w.addnstr(y, x, s, max(0, wd - x - 1), a)
        except curses.error: pass

def bar(f, n):
    f = max(0.0, min(f, 1.0)); k = int(round(f * n)); return "█" * k + "░" * (n - k)

def hm(m): return f"{m//60}h{m%60:02d}m"

def header(w, view):
    put(w, 0, 1, f"GEAR · {V['project']} · {V['milestone']}: {V['title']}", C["b"])
    put(w, 0, w.getmaxyx()[1] - 8, NOW, C["dim"])
    put(w, 1, 1, f"time   {bar(V['used_m']/V['eta_m'], 24)} {hm(V['used_m'])} of {V['eta_m']//60}h")
    put(w, 1, 48, f"projected finish {hm(V['projected_m'])} (slowest lane: {V['slow_lane']})",
        C["ok"] if V["projected_m"] <= V["eta_m"] else C["bad"])
    done = sum(1 for t in T if t[2] == "done"); left_m = sum(t[4] for t in T if t[2] != "done")
    put(w, 2, 1, f"tasks  {bar(done/len(T), 24)} {done}/{len(T)} done, {hm(left_m)} of estimates left")
    met = sum(d[0] for d in DONE)
    put(w, 2, 72, f"done sentence {met}/{len(DONE)}", C["warn"])
    x = 1; TABX.clear()
    for i, t in enumerate(["1 Now", "2 Graph", "3 Workload", "4 Decisions", "5 Time & tokens"], 1):
        put(w, 3, x, f" {t} ", curses.A_REVERSE if i == view else C["dim"]); TABX.append((x, x + len(t) + 2, i)); x += len(t) + 3

def lane_now(lane):
    cur = [t for t in T if t[1] == lane and t[2] in ("working",)]
    return cur[0] if cur else None

def idle_reason(n):
    q = [t for t in T if t[1] == n and t[2] in ("queued", "review", "merging")]
    rv = [t for t in q if t[2] in ("review", "merging")]
    if rv: return f"its {rv[0][0]} is in {rv[0][2]}; next task can start now (captain: deliver)"
    if not q: return "nothing left planned for this lane"
    nxt = q[0]; by = {t[0]: t for t in T}
    wait = [by[d] for d in nxt[9] if by[d][2] != "done"]
    if wait: return f"next {nxt[0]} waits for {wait[0][0]} ({wait[0][1]}, {wait[0][2]})"
    return f"next {nxt[0]} is ready but not delivered"

def view_now(w, sel):
    y = 5; put(w, y, 1, "LANES  (Enter: what this lane did and has left)", C["b"]); y += 1
    for i, n in enumerate(LANES):
        t = lane_now(n); a = curses.A_REVERSE if sel == i else 0
        done = sum(1 for x in T if x[1] == n and x[2] == "done")
        left = sum(1 for x in T if x[1] == n and x[2] not in ("done",))
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{n:<5}", C[LANE_COL[n]] | C["b"] | a)
        if t:
            over = t[5] > t[4]
            put(w, y, 9, f"{t[0]} {t[3][:32]:<32}", a)
            put(w, y, 52, f"{bar(t[5]/t[4], 10)} {t[5]}/{t[4]}m", C["warn"] if over else 0)
        else:
            put(w, y, 9, "idle: " + idle_reason(n), C["bad"] | a)
        lm = sum(x[4] for x in T if x[1] == n and x[2] != "done")
        put(w, y, 72, f"done {done} · left {left} ({lm}m)", C["dim"]); y += 1
    y += 1; put(w, y, 1, f"NEEDS A LOOK ({len(EXC)})", C["b"] | C["warn"]); y += 1
    for lane, tid, kind, why in EXC:
        put(w, y, 3, f"{tid} {lane:<5} {kind:<14}", C["warn"]); put(w, y, 29, why); y += 1
    y += 1; put(w, y, 1, "QUOTA", C["b"]); y += 1
    for p, win, u, r in QUOTA:
        put(w, y, 3, f"{p:<8}{win:<5}{bar(u/100, 14)} {u:>3}% used  resets {r}",
            C["bad"] if u >= 90 else C["warn"] if u >= 60 else C["ok"]); y += 1
    y += 1; put(w, y, 1, "DONE SENTENCE", C["b"]); y += 1
    for ok, s in DONE: put(w, y, 3, ("✔ " if ok else "· ") + s, C["ok"] if ok else C["dim"]); y += 1
    return len(LANES)

SYM = dict(done="●", merging="◎", review="◎", working="◉", queued="○")
def view_graph(w, sel):
    """Like `git log --graph`: dev on the left, one column per lane, newest on top."""
    y = 5
    cols = {n: 14 + 6 * i for i, n in enumerate(LANES)}
    put(w, y, 2, "dev", C["ok"] | C["b"]); put(w, y, 8, "rev", C["acc"] | C["b"])
    for n, x in cols.items(): put(w, y, x, n, C[LANE_COL[n]] | C["b"])
    y += 1
    order = ([t for t in T if t[2] == "queued"] + [t for t in T if t[2] == "working"] +
             [t for t in T if t[2] in ("review", "merging")] +
             sorted([t for t in T if t[2] == "done"], key=lambda t: t[10], reverse=True))
    textx = 14 + 6 * len(LANES) + 1
    seltask = order[sel] if sel < len(order) else None
    for i, t in enumerate(order):
        a = curses.A_REVERSE if sel == i else 0
        put(w, y, 3, "│", C["ok"])
        if t[2] in ("review", "merging"):
            x0 = cols[t[1]] + 1
            put(w, y, 10, "─" * (x0 - 10), C["acc"]); put(w, y, 9, "◎", C["acc"] | C["b"])
        for n, x in cols.items():
            # a lane line runs while the lane still has tasks below this row
            if any(o[1] == n for o in order[i+1:]): put(w, y, x + 1, "│", C[LANE_COL[n]])
        x = cols[t[1]] + 1
        put(w, y, x, SYM[t[2]], C[STAGE_COL[t[2]]] | C["b"])
        if t[2] == "done":
            put(w, y, 3, "●", C["ok"] | C["b"])
            put(w, y, 4, "─" * (x - 4), C["ok"])
            for n2, x2 in cols.items():
                if x2 + 1 < x and any(o[1] == n2 for o in order[i+1:]): put(w, y, x2 + 1, "┼", C[LANE_COL[n2]])
        put(w, y, textx, f"{t[0]}", C[LANE_COL[t[1]]] | a)
        put(w, y, textx + 6, f"{t[1]:<5}{t[3][:34]:<35}", a)
        info = {"done": f"merged {t[10]} · {t[5]}/{t[4]}m" + (f" · {t[6]} rounds" if t[6] > 1 else ""),
                "working": f"working {t[5]}/{t[4]}m", "review": "in review", "merging": "merging",
                "queued": f"queued · {t[4]}m"}[t[2]]
        put(w, y, textx + 47, info, C[STAGE_COL[t[2]]] if t[2] != "done" else C["dim"])
        if t[2] == "done": put(w, y, 9, "✓", C["ok"] | C["b"])
        if t[6] > 1: put(w, y, 8, "↺", C["warn"] | C["b"])
        mark = ""
        if seltask and t[0] in seltask[9]: mark = "◂ needed by " + seltask[0]
        elif seltask and seltask[0] in t[9]: mark = "▸ waits for " + seltask[0]
        if mark: put(w, y, textx + 80, mark, C["warn"] | C["b"])
        elif t[9]: put(w, y, textx + 80, "needs " + ",".join(t[9]), C["dim"])
        y += 1
    y += 1; put(w, y, 2, "○ queued  ◉ working  ◎ in review (rev column)  ↺ reworked  ✓● reviewed and merged into dev   select a row: its deps light up", C["dim"])
    return len(order)

def view_workload(w, sel):
    y = 5; put(w, y, 1, "WORK DONE AND LEFT, BY LANE  (minutes of estimate)", C["b"]); y += 1
    tot_d = tot_l = 0
    for i, n in enumerate(LANES):
        d = sum(t[4] for t in T if t[1] == n and t[2] == "done")
        l = sum(t[4] for t in T if t[1] == n and t[2] != "done")
        nd = sum(1 for t in T if t[1] == n and t[2] == "done"); nl = sum(1 for t in T if t[1] == n and t[2] != "done")
        tot_d += d; tot_l += l
        k = int(round(d / 8)); r = int(round(l / 8))
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{n:<5}", C[LANE_COL[n]] | C["b"] | (curses.A_REVERSE if sel == i else 0))
        put(w, y, 9, "█" * k, C["ok"]); put(w, y, 9 + k, "░" * r, C["dim"])
        put(w, y, 40, f"done {nd} ({d}m) · left {nl} ({l}m)"); y += 1
    y += 1; put(w, y, 3, f"all lanes: done {tot_d}m · left {tot_l}m · at the measured pace the last lane ends at {hm(V['projected_m'])}", C["b"]); y += 2
    put(w, y, 1, "BURN-UP  (minutes of estimate merged, per voyage hour)", C["b"]); y += 1
    top = max(BURN + [tot_d + tot_l]); H = 8
    for row in range(H, 0, -1):
        lvl = top * row / H
        line = "".join(" ██ " if b >= lvl else "    " for b in BURN)
        put(w, y, 8, line, C["ok"]); y += 1
    put(w, y - H, 8 + 4 * len(BURN), f"  ─ total {tot_d + tot_l}m", C["dim"])
    put(w, y, 8, "".join(f" h{i:<2}" for i in range(len(BURN))), C["dim"])
    return len(LANES)

def view_decisions(w, sel):
    y = 5; put(w, y, 1, "DECISIONS MADE  (★ = worth a look at landing)", C["b"]); y += 1
    for i, (t, k, what, why, star) in enumerate(DECISIONS):
        a = curses.A_REVERSE if sel == i else 0
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{t} {'★' if star else ' '} {k:<9}", (C["warn"] if star else 0) | a); put(w, y, 22, what, a); y += 1
        if why: put(w, y, 22, "why: " + why, C["dim"]); y += 1
    y += 1; put(w, y, 1, "FRICTION REPORTS FROM AGENTS", C["b"]); y += 1
    for tid, lane, st, s in FRICTION:
        put(w, y, 3, f"{tid} {lane:<5}", C[LANE_COL[lane]]); put(w, y, 15, f"{st:<7}", C["warn"] if st == "unread" else C["dim"]); put(w, y, 23, s); y += 1
    return len(DECISIONS)

def view_cost(w, sel):
    y = 5; put(w, y, 3, f"{'task':<7}{'lane':<6}{'est':>5}{'act':>6}{'ratio':>7}{'rounds':>8}{'tokens':>9}{'cost':>8}", C["dim"]); y += 1
    done = [t for t in T if t[5]]
    for i, t in enumerate(done):
        a = curses.A_REVERSE if sel == i else 0; r = t[5] / t[4]
        put(w, y, 1, "›" if sel == i else " ", C["acc"])
        put(w, y, 3, f"{t[0]:<7}", a); put(w, y, 10, f"{t[1]:<6}", C[LANE_COL[t[1]]])
        put(w, y, 16, f"{t[4]:>4}m{t[5]:>5}m", a)
        put(w, y, 27, f"{r:>6.1f}x", C["bad"] if r > 1.5 else C["warn"] if r > 1 else C["ok"])
        put(w, y, 34, f"{t[6]:>8}{t[7]:>8}k  ${t[8]:.2f}", a); y += 1
    y += 1; put(w, y, 3, f"so far: {sum(t[7] for t in done)/1000:.1f}M tokens, ${sum(t[8] for t in done):.2f}", C["b"])
    return len(done)

def lane_panel(w, lane):
    rows = [(f"lane {lane} · {MODEL}", C[LANE_COL[lane]] | C["b"]), ("", 0), ("DONE", C["b"])]
    for t in [t for t in T if t[1] == lane and t[2] == "done"]:
        rows.append((f"  ● {t[0]} {t[3][:30]:<30} {t[5]}/{t[4]}m" + (f" {t[6]}r" if t[6] > 1 else ""), C["ok"]))
    rows += [("", 0), ("NOW", C["b"])]
    for t in [t for t in T if t[1] == lane and t[2] in ("working", "review", "merging")]:
        rows.append((f"  {SYM[t[2]]} {t[0]} {t[3][:30]:<30} {t[2]}", C[STAGE_COL[t[2]]]))
    rows += [("", 0), ("LEFT, in order", C["b"])]
    left = [t for t in T if t[1] == lane and t[2] == "queued"]
    for t in left:
        rows.append((f"  ○ {t[0]} {t[3][:30]:<30} {t[4]}m" + (f"  needs {','.join(t[9])}" if t[9] else ""), 0))
    rows += [("", 0), (f"left: {len(left)} tasks, {sum(t[4] for t in left)}m of estimates", C["dim"])]
    h, wd = w.getmaxyx(); bw = min(72, wd - 2); bh = len(rows) + 3
    win = curses.newwin(bh, bw, 5, max(0, wd - bw - 1)); win.erase(); win.box()
    for i, (s, a) in enumerate(rows): put(win, i + 1, 2, s, a)
    put(win, bh - 1, 2, " any key: close ", C["dim"]); win.refresh(); win.getch()

def task_panel(w, t):
    by = {x[0]: x for x in T}
    rows = [(f"{t[0]} · lane {t[1]} · {t[2]}", C[LANE_COL[t[1]]] | C["b"]), (t[3], 0), ("", 0),
            (f"estimate {t[4]}m · actual {t[5]}m" + (" · over estimate" if t[5] > t[4] else ""), C["warn"] if t[5] > t[4] else 0),
            ("", 0), ("ROUNDS", C["b"])]
    if t[6] == 0: rows.append(("  not started", C["dim"]))
    else:
        rows.append((f"  dev 1      {t[1]:<6} {int(t[5]*0.7)}m   {int(t[7]*0.6)}k tokens", 0))
        if t[2] in ("review", "merging", "done") or t[6] > 1: rows.append(("  review 1   codex   7m   300k tokens" + ("  → fail: rework" if t[6] > 1 else "  → pass"), 0))
        if t[6] > 1:
            rows.append((f"  rework 2   {t[1]:<6} {int(t[5]*0.3)}m   {int(t[7]*0.2)}k tokens", 0))
            rows.append(("  review 2   codex   5m   180k tokens" + ("  → pass" if t[2] == "done" else "  (running)"), 0))
    rows += [("", 0), ("DEPENDS ON", C["b"])]
    rows += [(f"  {d} {by[d][1]:<5} {by[d][3][:30]}  ({by[d][2]})", C[STAGE_COL[by[d][2]]]) for d in t[9]] or [("  nothing", C["dim"])]
    rows += [("", 0), ("NEEDED BY", C["b"])]
    rows += [(f"  {x[0]} {x[1]:<5} {x[3][:30]}  ({x[2]})", C[STAGE_COL[x[2]]]) for x in T if t[0] in x[9]] or [("  nothing", C["dim"])]
    if t[2] == "done": rows += [("", 0), (f"merged {t[10]} · friction report: " + ("yes (unread)" if t[0] == "T-139" else "none"), C["dim"])]
    h, wd = w.getmaxyx(); bw = min(72, wd - 2); bh = len(rows) + 3
    win = curses.newwin(bh, bw, 5, max(0, wd - bw - 1)); win.erase(); win.box()
    for i, (s_, a) in enumerate(rows): put(win, i + 1, 2, s_, a)
    put(win, bh - 1, 2, " any key: close ", C["dim"]); win.refresh(); win.getch()

def main(w):
    colors(); curses.curs_set(0); view, sel = 1, 0
    curses.mousemask(curses.ALL_MOUSE_EVENTS | curses.REPORT_MOUSE_POSITION); w.keypad(True)
    views = {1: view_now, 2: view_graph, 3: view_workload, 4: view_decisions, 5: view_cost}
    while True:
        w.erase(); header(w, view); n = views[view](w, sel)
        put(w, w.getmaxyx()[0] - 1, 1, "←→ / Tab / 1-5 / click: views · ↑↓ select · Enter: a lane's work / a task's rounds · q quit      PROTOTYPE v3 · fake data", C["dim"])
        w.refresh(); k = w.getch()
        if k in (ord("q"), 27): break
        if k == curses.KEY_MOUSE:
            try: _, mx, my, _, bs = curses.getmouse()
            except curses.error: continue
            if my == 3:
                for x0, x1, i in TABX:
                    if x0 <= mx <= x1: view, sel = i, 0
            elif my >= 6: sel = min(max(0, my - 6), n - 1)
            continue
        if k in (curses.KEY_RIGHT, ord("l"), 9): view, sel = view % 5 + 1, 0
        elif k in (curses.KEY_LEFT, ord("h"), curses.KEY_BTAB): view, sel = (view - 2) % 5 + 1, 0
        elif ord("1") <= k <= ord("5"): view, sel = k - ord("0"), 0
        elif k in (curses.KEY_DOWN, ord("j")): sel = min(sel + 1, n - 1)
        elif k in (curses.KEY_UP, ord("k")): sel = max(sel - 1, 0)
        elif k in (10, 13, curses.KEY_ENTER):
            if view in (1, 3): lane_panel(w, LANES[sel])
            elif view == 2:
                order = ([t for t in T if t[2] == "queued"] + [t for t in T if t[2] == "working"] +
                         [t for t in T if t[2] in ("review", "merging")] +
                         sorted([t for t in T if t[2] == "done"], key=lambda t: t[10], reverse=True))
                task_panel(w, order[sel])

curses.wrapper(main)
