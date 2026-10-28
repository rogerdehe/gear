#!/usr/bin/env python3
"""GEAR board prototype v3 for M5 (fake data, curses, no dependencies).

Keys: 1-5 or Tab / <- -> views · [ ] older / newer voyage · up/down select ·
Enter open or close · q quit.
"""
import curses

# ---------------------------------------------------------------- fake data
# Every voyage of the project, newest first. A landed voyage keeps its data.
VOYAGES = [
    dict(id="M5", state="sailing", title="small tasks, the board, the goal rule",
         done="a small task lands without review in under 10 min; the board shows every voyage "
              "and uses the full width; a departed goal changes only with the user's yes",
         bar="DELIVERABLE", eta=480, used=212, projected=455,
         goal_changed=None,
         stations=[("S1", "small-task procedure merged", "done", 180),
                   ("S2", "board views to the new prototype", "now", 330),
                   ("S3", "goal lock + v0.5.0 released", "next", 480)],
         distance=["small task under 10 min: measured 14 min (T-262)",
                   "board shows every voyage: done",
                   "goal lock: not started"]),
    dict(id="M4.2", state="landed", title="fixes from real use",
         done="every M4.2 task merged with no hand fix; init --check changes nothing; "
              "no run-dry item while no lane can start work; released as v0.4.8",
         bar="DELIVERABLE", eta=360, used=256, projected=256,
         goal_changed=None,
         stations=[("S1", "29 tasks merged", "done", 256), ("S2", "v0.4.8 released", "done", 360)],
         distance=["no hand fix: NOT met (3 hand fixes)", "init --check: met (T-228)",
                   "no run-dry item: met in code (T-232)", "v0.4.8: released"]),
    dict(id="M4.1", state="landed", title="two fixes another captain needed",
         done="T-226 and T-227 merged and released as v0.4.7",
         bar="FASTEST", eta=60, used=75, projected=75, goal_changed=None,
         stations=[("S1", "v0.4.7 released", "done", 75)],
         distance=["both tasks merged, v0.4.7 released: met"]),
    dict(id="M4", state="landed", title="one seat model, a voyage the user can review",
         done="reviewer seats kept and released; a lane rebinds; decisions reviewed at landing; v0.4.0",
         bar="DELIVERABLE", eta=720, used=1980, projected=1980,
         goal_changed=("2026-10-27 20:03", "five clauses", "six clauses (Herdr restart added)"),
         stations=[("S1", "v0.4.0", "done", 600), ("S2", "v0.4.4", "done", 1980)],
         distance=["5 of 6 clauses met; Herdr restart: moved to M5"]),
]

LANES = {"M5": ["hull", "mast", "keel"], "M4.2": ["hull", "mast"], "M4.1": ["hull", "keel"],
         "M4": ["hull", "deck", "keel", "mast"]}

# Task stages, in time order (one per task), with the words the board shows.
STAGE = {
    "deps": "waits for {dep} to merge", "queued": "queued", "delivered": "delivered, not started",
    "working": "working", "asking": "waits for the captain's answer",
    "handed": "handed in, waits for a reviewer", "review": "in review",
    "verdict": "waits for the captain's verdict", "sentback": "sent back, rework queued",
    "rework": "reworking (round {round})", "passed": "passed, waits to merge",
    "merging": "merging (gate running)", "mergefail": "merge failed, waits for the captain",
    "merged": "merged", "failed": "failed (rounds used up)", "moved": "moved to {to}",
}
# id, lane, stage, title, est, actual, round, start, merge (minutes from departure), deps,
# dev tokens k, dev $, review tokens k, review $, marks
T = {
    "M5": [
        ("T-257", "hull", "merged",  "small task: plan marks it",          20, 18, 1,   5,  31, [],        410, .16, 120, .09, ""),
        ("T-258", "mast", "merged",  "board: voyage list and [ ] keys",    40, 44, 1,   5,  58, [],        820, .31, 260, .19, ""),
        ("T-259", "keel", "merged",  "goal line: station and distance",    30, 27, 1,   8,  46, [],        520, .20, 150, .11, ""),
        ("T-260", "hull", "merged",  "small task: no review, accept proof", 30, 41, 2,  32, 102, ["T-257"], 990, .37, 410, .30, ""),
        ("T-261", "mast", "merged",  "board: full-width frame",            45, 52, 1,  59, 128, ["T-258"], 1010, .38, 300, .22, ""),
        ("T-262", "keel", "review",  "goal lock: GOAL_LOCKED at apply",    40, 35, 1,  47, None, [],        640, .24, 90,  .07, ""),
        ("T-263", "hull", "rework",  "small task: captain settles it",     25, 49, 2, 103, None, ["T-260"], 760, .29, 200, .15, "past its budget"),
        ("T-264", "mast", "working", "board: Time & tokens by lane",       40, 22, 1, 129, None, ["T-261"], 380, .14, 0,   0,   ""),
        ("T-265", "keel", "deps",    "goal-change line with the user's words", 35, 0, 0, None, None, ["T-262"], 0, 0, 0, 0, ""),
        ("T-266", "mast", "queued",  "board: graph as a git graph",        60,  0, 0, None, None, ["T-261"], 0, 0, 0, 0, ""),
        ("T-267", "hull", "queued",  "release v0.5.0",                     15,  0, 0, None, None, ["T-263", "T-266"], 0, 0, 0, 0, ""),
    ],
    "M4.2": [
        ("T-228", "mast", "merged", "init --check validates, starts nothing", 30, 41, 2, 0, 52, [], 1200, .45, 380, .28, ""),
        ("T-236", "hull", "merged", "a dependency counts when merged",     20, 24, 1, 2, 30, [], 610, .23, 150, .11, ""),
        ("T-241", "hull", "merged", "urgent message mid tool call",        25, 110, 2, 30, 158, [], 2400, .9, 700, .52, ""),
        ("T-248", "hull", "merged", "the doorbell says what to run",       10, 112, 3, 160, 254, [], 1900, .71, 640, .47, ""),
        ("T-252", "mast", "merged", "branch templates in gear-model",      25, 30, 1, 60, 96, [], 700, .26, 210, .16, ""),
        ("T-253", "hull", "merged", "engine uses the templates",           30, 70, 2, 100, 215, ["T-252"], 1500, .56, 420, .31, ""),
        ("T-256", "mast", "merged", "two projects need {project}",         25, 60, 2, 150, 250, ["T-253"], 1100, .41, 380, .28, ""),
        ("T-182", "mast", "moved",  "push race re-merge",                  20, 0, 0, None, None, [], 0, 0, 0, 0, "M5"),
    ],
    "M4.1": [
        ("T-226", "hull", "merged", "a restart takes the current config", 30, 18, 1, 0, 22, [], 400, .15, 120, .09, ""),
        ("T-227", "keel", "merged", "silent seats are reported",          30, 73, 2, 0, 75, [], 1300, .49, 300, .22, ""),
    ],
    "M4": [
        ("T-143", "hull", "merged", "one seat model",                     60, 240, 3, 0, 300, [], 5200, 1.9, 900, .66, ""),
        ("T-155", "deck", "merged", "board: now view",                    45, 50, 1, 10, 70, [], 900, .34, 200, .15, ""),
        ("T-174", "keel", "merged", "a lane rebinds",                     40, 44, 1, 20, 80, [], 800, .3, 200, .15, ""),
        ("T-176", "mast", "moved",  "engine follows Herdr restarts",      40, 0, 0, None, None, [], 0, 0, 0, 0, "M5"),
    ],
}
DECISIONS = {
    "M5": dict(course=["★ 14:05 T-262 contract v2: GOAL_LOCKED also covers station lines (user agreed 13:58)"],
               waiting=["T-263: the captain's rework scope (hull is past its budget)",
                        "T-262: review by review-3, 12 min in"],
               counts="12 decisions · 2 reworks · 1 contract change · 3 friction reports (2 closed)"),
    "M4.2": dict(course=["★ 18:15 the gear/ folder layout: one folder per voyage (user)",
                         "★ 19:00 voyage report: done (1) not met, 3 hand fixes"],
                 waiting=[], counts="45 decisions · 5 reworks · 3 hand fixes · 21 friction reports (all closed)"),
    "M4.1": dict(course=["14:05 voyage report: 75m against 1h"], waiting=[], counts="6 decisions · 1 rework"),
    "M4": dict(course=["★ 20:03 the user kept the full scope; six cut tasks named"], waiting=[],
               counts="210 decisions · 31 reworks · 58 friction reports"),
}
QUOTA = [("tokenrhythm", "lanes", 41, "22:00"), ("codex", "review", 63, "Oct 30")]
VIEWS = ["Now", "Graph", "Workload", "Decisions", "Time & tokens"]

# ---------------------------------------------------------------- helpers
def hm(m):
    return f"{m // 60}h{m % 60:02d}" if m >= 60 else f"{m}m"

def words(task):
    tid, lane, st, title, est, act, rnd, *_ = task
    s = STAGE[st]
    return s.format(dep=", ".join(task[9]) or "-", round=rnd, to=task[14] or "-")

def wrap(text, width):
    out, line = [], ""
    for w in text.split():
        if len(line) + len(w) + 1 > width and line:
            out.append(line)
            line = w
        else:
            line = (line + " " + w).strip()
    if line:
        out.append(line)
    return out or [""]


class Board:
    def __init__(self, scr):
        self.scr = scr
        self.view = 0
        self.vi = 0          # selected voyage, 0 = newest
        self.sel = 0
        self.open = set()    # expanded lanes (Time & tokens) / tasks
        curses.curs_set(0)
        curses.use_default_colors()
        pal = [curses.COLOR_CYAN, curses.COLOR_MAGENTA, curses.COLOR_YELLOW, curses.COLOR_GREEN]
        for i, c in enumerate(pal, start=1):
            curses.init_pair(i, c, -1)
        curses.init_pair(10, curses.COLOR_RED, -1)
        curses.init_pair(11, curses.COLOR_GREEN, -1)
        curses.init_pair(12, curses.COLOR_BLACK, curses.COLOR_CYAN)

    # -- drawing primitives
    def put(self, y, x, s, a=0):
        h, w = self.scr.getmaxyx()
        if 0 <= y < h and x < w:
            try:
                self.scr.addnstr(y, x, s, max(0, w - x - 1), a)
            except curses.error:
                pass

    def lane_attr(self, lane):
        lanes = LANES[self.v["id"]]
        return curses.color_pair(1 + lanes.index(lane) % 4) if lane in lanes else 0

    @property
    def v(self):
        return VOYAGES[self.vi]

    # -- frame: voyage list, goal, stations, tabs
    def header(self, w):
        x = 1
        self.put(0, x, "GEAR · gear ", curses.A_BOLD)
        x += 13
        for i, v in enumerate(VOYAGES):
            label = f" {v['id']} {'⛵' if v['state'] == 'sailing' else '⚓'} "
            self.put(0, x, label, curses.color_pair(12) | curses.A_BOLD if i == self.vi else curses.A_DIM)
            x += len(label) + 1
        self.put(0, x + 1, "[ older  ] newer", curses.A_DIM)
        v = self.v
        state = "⛵ sailing" if v["state"] == "sailing" else "⚓ landed"
        self.put(1, 1, f"{v['id']} · {v['title']}", curses.A_BOLD)
        self.put(1, max(1, w - len(state) - 3), state, curses.A_BOLD)
        # the goal, boxed and bright: the one thing the captain and the user must not lose
        gw = w - 4
        gold = curses.color_pair(3) | curses.A_BOLD
        self.put(2, 1, "╔═ GOAL " + "═" * max(0, gw - 8) + "╗", gold)
        y = 3
        for line in wrap(v["done"][0].upper() + v["done"][1:], gw - 4):
            self.put(y, 1, "║ " + line.ljust(gw - 3) + "║", gold)
            y += 1
        self.put(y, 1, "╚" + "═" * (gw - 1) + "╝", gold)
        y += 1
        if v["goal_changed"]:
            at, before, after = v["goal_changed"]
            self.put(y, 1, f"{'Goal changed':<14}{at}: {before} -> {after} (the user agreed; see Decisions)",
                     curses.color_pair(10) | curses.A_BOLD)
            y += 1
        label = curses.A_DIM
        self.put(y, 1, "Goal check", label)
        for part in v["distance"]:
            bad = "NOT" in part or "not started" in part or "measured" in part
            self.put(y, 15, ("✗ " if bad else "✓ ") + part, curses.color_pair(10) if bad else curses.color_pair(11))
            y += 1
        self.put(y, 1, "Milestones", label)
        for sid, what, st, at in v["stations"]:
            if st == "done":
                txt, a = f"✓ {sid} {what} — reached at {hm(at)}", curses.color_pair(11)
            elif st == "now":
                txt, a = f"▶ {sid} {what} — now, planned by {hm(at)}", curses.A_BOLD
            else:
                txt, a = f"· {sid} {what} — planned by {hm(at)}", curses.A_DIM
            self.put(y, 15, txt, a)
            y += 1
        # time as a bar: █ used, ◆ expected finish, │ planned end; then the same in words
        self.put(y, 1, "Time", label)
        span = max(v["eta"], v["projected"], v["used"])
        bw = max(20, min(50, w - 90))
        pos = lambda m: min(bw - 1, m * bw // span)
        cells = ["░"] * bw
        for i in range(pos(v["used"])):
            cells[i] = "█"
        end = pos(v["eta"])
        cells[end] = "│"
        if v["state"] == "sailing":
            cells[pos(v["projected"])] = "◆"
        early = v["eta"] - (v["projected"] if v["state"] == "sailing" else v["used"])
        ok = early >= 0
        self.put(y, 15, "".join(cells), curses.color_pair(11) if ok else curses.color_pair(10))
        if v["state"] == "sailing":
            when = (f"used {hm(v['used'])} · planned {hm(v['eta'])} · expected {hm(v['projected'])}, "
                    + (f"{hm(early)} ahead of plan" if ok else f"{hm(-early)} behind plan"))
        else:
            when = (f"took {hm(v['used'])} · planned {hm(v['eta'])} · "
                    + (f"{hm(early)} under plan" if ok else f"{hm(-early)} over plan"))
        self.put(y, 17 + bw, when, curses.A_BOLD)
        y += 1
        self.put(y, 15, "█ used   ◆ expected finish   │ planned end", curses.A_DIM)
        y += 1
        # tasks of this voyage only: merged, in progress, not started (moved ones are named apart)
        tasks = [t for t in T[v["id"]] if t[2] != "moved"]
        merged = sum(1 for t in tasks if t[2] == "merged")
        waiting = sum(1 for t in tasks if t[2] in ("queued", "deps"))
        going = len(tasks) - merged - waiting
        moved = [t[0] for t in T[v["id"]] if t[2] == "moved"]
        self.put(y, 1, "Tasks", label)
        n = max(1, len(tasks))
        a, b = merged * bw // n, going * bw // n
        self.put(y, 15, "█" * a, curses.color_pair(11))
        self.put(y, 15 + a, "▓" * b, curses.color_pair(3))
        self.put(y, 15 + a + b, "░" * (bw - a - b), curses.A_DIM)
        txt = f"{merged} of {len(tasks)} merged · {going} in progress · {waiting} not started"
        if moved:
            txt += f" · moved out: {', '.join(moved)}"
        self.put(y, 17 + bw, txt, curses.A_BOLD)
        y += 1
        self.put(y, 15, "█ merged   ▓ in progress   ░ not started", curses.A_DIM)
        y += 1
        bars = {"FASTEST": "only what breaks the chain or loses data blocks a task",
                "DELIVERABLE": "errors in normal use block a task; edge cases are logged",
                "STRICT": "severe issues block a task; medium ones are the captain's call"}
        self.put(y, 1, "Quality bar", label)
        self.put(y, 15, f"{v['bar']} — {bars.get(v['bar'], '')}")
        y += 1
        x = 1
        for i, name in enumerate(VIEWS):
            label = f" {i + 1} {name} "
            self.put(y, x, label, curses.A_REVERSE if i == self.view else 0)
            x += len(label) + 1
        self.put(y + 1, 0, "─" * (w - 1), curses.A_DIM)
        return y + 2

    # -- 1 Now: lanes on the left, needs a look + quota on the right
    def now(self, top, w, h):
        tasks = T[self.v["id"]]
        left_w = w if w < 140 else w * 11 // 20
        y = top
        self.put(y, 1, "LANES", curses.A_BOLD)
        y += 1
        for i, lane in enumerate(LANES[self.v["id"]]):
            mine = [t for t in tasks if t[1] == lane]
            cur = next((t for t in mine if t[2] not in ("merged", "queued", "deps", "moved")), None)
            done = [t for t in mine if t[2] == "merged"]
            left = [t for t in mine if t[2] in ("queued", "deps")]
            a = curses.A_REVERSE if i == self.sel else 0
            self.put(y, 1, f"{lane:<6}", self.lane_attr(lane) | curses.A_BOLD | a)
            if cur:
                txt = f"{cur[0]} {cur[3]} — {words(cur)} · {cur[5]}/{cur[4]}m"
                if cur[15 - 1] and cur[2] != "moved":
                    txt += f" · ⚠ {cur[14]}"
            elif self.v["state"] == "landed":
                txt = "retired at the landing"
            else:
                txt = "idle: " + ("next task waits for " + ", ".join(left[0][9]) if left and left[0][9] else "nothing planned")
            self.put(y, 9, txt[: left_w - 10], a)
            y += 1
            n = max(1, len(mine))
            bar = "█" * (len(done) * 12 // n) + "░" * (12 - len(done) * 12 // n)
            self.put(y, 9, bar, self.lane_attr(lane))
            self.put(y, 22, f"{len(done)} of {len(mine)} tasks done ({hm(sum(t[5] for t in done))}) · "
                            f"{len(left)} not started ({hm(sum(t[4] for t in left))} estimated)", curses.A_DIM)
            y += 2
        rx, ry = (1, y + 1) if w < 140 else (left_w + 3, top)
        self.put(ry, rx, "NEEDS A LOOK (open tasks only)", curses.A_BOLD)
        ry += 1
        looks = [t for t in tasks if t[2] not in ("merged", "moved") and (t[5] > t[4] or t[6] > 1)]
        if not looks:
            self.put(ry, rx, "nothing", curses.A_DIM)
            ry += 1
        for t in looks:
            self.put(ry, rx, f"{t[0]} {t[1]}: {words(t)} · {t[5]}/{t[4]}m · round {t[6]}", curses.color_pair(10))
            ry += 1
        ry += 1
        self.put(ry, rx, "QUOTA", curses.A_BOLD)
        ry += 1
        for prov, role, used, reset in QUOTA:
            bar = "█" * (used // 10) + "░" * (10 - used // 10)
            self.put(ry, rx, f"{prov:<12} {role:<7} {bar} {used}% · resets {reset}")
            ry += 1

    # -- 2 Graph: dev as the main line, each task a coloured branch from where its work starts
    def graph(self, top, w, h):
        tasks = T[self.v["id"]]
        lanes = LANES[self.v["id"]]
        col = {lane: 7 + 6 * i for i, lane in enumerate(lanes)}
        info_x = col[lanes[-1]] + 5
        tw = max(24, min(44, w - info_x - 40))
        y = top
        self.put(y, 1, "dev", curses.A_BOLD)
        for lane in lanes:
            self.put(y, col[lane] - 1, lane[:4], self.lane_attr(lane) | curses.A_BOLD)
        self.put(y, info_x, f"{'task':<7}{'what':<{tw}}  {'stage':<34}time", curses.A_DIM)
        y += 1
        events = []
        for t in tasks:
            if t[7] is not None:
                events.append((t[7], 0, t))      # starts: branch leaves dev
            if t[8] is not None:
                events.append((t[8], 1, t))      # merges: branch joins dev
        events.sort(key=lambda e: (e[0], e[1]), reverse=True)
        waiting = [t for t in tasks if t[7] is None and t[2] != "moved"]
        open_ids = {t[0] for t in tasks if t[7] is not None and t[8] is None}
        live = set(open_ids)                    # branches drawn as a vertical line on this row

        def row(cells, t, text, attr):
            nonlocal y
            if y >= h - 3:
                return
            self.put(y, 2, "│", curses.A_DIM)
            for x, ch, a in cells:
                self.put(y, x, ch, a)
            if text:
                self.put(y, info_x, text, attr)
            y += 1

        def verticals(skip=None):
            return [(col[o[1]], "│", self.lane_attr(o[1])) for o in tasks if o[0] in live and o[0] != skip]

        def info(t):
            stage = words(t)
            time_ = f"{t[5]}/{t[4]}m" + (f" · {t[6]} rounds" if t[6] > 1 else "")
            return f"{t[0]:<7}{t[3][:tw]:<{tw}}  {stage[:34]:<34}{time_}"

        # not started yet: a dashed stub in its lane, naming what it waits for
        for t in waiting:
            a = self.lane_attr(t[1])
            row(verticals() + [(col[t[1]], "○", a | curses.A_BOLD)], t, info(t), curses.A_DIM)
            row(verticals() + [(col[t[1]], "┆", a)], t, "", 0)
        for at, kind, t in events:
            c, a = col[t[1]], self.lane_attr(t[1])
            if kind == 1:   # merge: ● on dev, the branch comes in from its lane
                cells = verticals(skip=t[0]) + [(2, "●", curses.A_BOLD)]
                cells += [(x, "─", a) for x in range(3, c)] + [(c, "╯", a)]
                live.add(t[0])
                row(cells, t, info(t) + f"  merged +{hm(at)}", 0)
            else:           # start: the branch leaves dev here
                if t[0] in open_ids:   # still open: its node sits on the row it started
                    node = {"working": "◉", "review": "◎", "rework": "↺", "handed": "◎",
                            "passed": "◍", "merging": "◍"}.get(t[2], "◉")
                    row(verticals(skip=t[0]) + [(c, node, a | curses.A_BOLD)], t, info(t), curses.A_BOLD)
                cells = verticals(skip=t[0]) + [(2, "├", curses.A_DIM)]
                cells += [(x, "─", a) for x in range(3, c)] + [(c, "╮", a)]
                live.discard(t[0])
                after = f" after {', '.join(t[9])}" if t[9] else ""
                row(cells, t, f"{'':<7}starts +{hm(at)}{after}", curses.A_DIM)
        self.put(h - 2, 1, "● merged into dev   ◉ working   ◎ in review   ↺ reworking   ○┆ not started   "
                           "a branch leaves dev where its work starts and joins it where it merges", curses.A_DIM)

    # -- 3 Workload: is the work balanced, and when does each lane end?
    def workload(self, top, w, h):
        tasks = T[self.v["id"]]
        lanes = LANES[self.v["id"]]
        y = top
        self.put(y, 1, "Is the work balanced? Each lane: what it did, what it has left (estimates), when it ends.",
                 curses.A_BOLD)
        y += 2
        bar_w = max(20, min(70, w - 75))
        scale = max(1, max(sum(t[5] for t in tasks if t[1] == l) + sum(t[4] for t in tasks if t[1] == l and t[2] != "merged")
                           for l in lanes))
        ends = {}
        for lane in lanes:
            mine = [t for t in tasks if t[1] == lane]
            done = sum(t[5] for t in mine if t[2] == "merged")
            left = sum(max(0, t[4] - t[5]) if t[2] not in ("merged", "moved") else 0 for t in mine)
            ends[lane] = self.v["used"] + left
            d = done * bar_w // scale
            l = left * bar_w // scale
            self.put(y, 1, f"{lane:<6}", self.lane_attr(lane) | curses.A_BOLD)
            self.put(y, 8, "█" * d, self.lane_attr(lane))
            self.put(y, 8 + d, "░" * l, self.lane_attr(lane))
            self.put(y, 10 + bar_w, f"did {hm(done):>5} · left {hm(left):>5} · ends at +{hm(ends[lane])}")
            y += 2
        first, last = min(ends, key=ends.get), max(ends, key=ends.get)
        gap = ends[last] - ends[first]
        verdict = (f"Balanced: every lane ends within {hm(gap)}." if gap <= 30 else
                   f"Not balanced: {last} ends {hm(gap)} after {first}; move {last}'s next startable task "
                   f"(the whole wave, by paths) to {first}.")
        self.put(y, 1, verdict, curses.color_pair(11) if gap <= 30 else curses.color_pair(10))
        y += 1
        self.put(y, 1, f"The voyage ends at +{hm(max(ends.values()))} against an eta of {hm(self.v['eta'])}.",
                 curses.A_DIM)

    # -- 4 Decisions: a summary; the full log stays in the store and the plan
    def decisions(self, top, w, h):
        d = DECISIONS[self.v["id"]]
        y = top
        self.put(y, 1, "WHAT CHANGED THE COURSE", curses.A_BOLD)
        y += 1
        for line in d["course"] or ["nothing"]:
            for part in wrap(line, w - 4):
                self.put(y, 3, part)
                y += 1
        y += 1
        self.put(y, 1, "WAITING ON SOMEONE", curses.A_BOLD)
        y += 1
        for line in d["waiting"] or ["nothing"]:
            self.put(y, 3, line)
            y += 1
        y += 1
        self.put(y, 1, d["counts"], curses.A_DIM)
        y += 1
        self.put(y, 1, "The full log: gear report voyage --voyage " + self.v["id"], curses.A_DIM)

    # -- 5 Time & tokens: grouped by lane, dev and review apart; Enter expands a lane
    def tokens(self, top, w, h):
        tasks = T[self.v["id"]]
        lanes = LANES[self.v["id"]]
        y = top
        hdr = f"{'lane / task':<44}{'time':>12}{'dev tokens':>14}{'dev $':>9}{'review tokens':>16}{'review $':>10}{'total $':>10}"
        self.put(y, 1, hdr, curses.A_BOLD)
        y += 1
        tot = [0, 0, 0, 0]
        for i, lane in enumerate(lanes):
            mine = [t for t in tasks if t[1] == lane]
            s = [sum(t[10] for t in mine), sum(t[11] for t in mine), sum(t[12] for t in mine), sum(t[13] for t in mine)]
            tot = [a + b for a, b in zip(tot, s)]
            open_ = lane in self.open
            a = curses.A_REVERSE if i == self.sel else 0
            label = f"{'▾' if open_ else '▸'} {lane} ({len(mine)} tasks)"
            self.put(y, 1, f"{label:<44}{hm(sum(t[5] for t in mine)):>12}{s[0]:>13}k{s[1]:>9.2f}{s[2]:>15}k{s[3]:>10.2f}"
                           f"{s[1] + s[3]:>10.2f}", self.lane_attr(lane) | curses.A_BOLD | a)
            y += 1
            if open_:
                for t in mine:
                    rounds = f" · {t[6]} rounds" if t[6] > 1 else ""
                    name = f"   {t[0]} {t[3]}"[:40]
                    self.put(y, 1, f"{name:<44}{(str(t[5]) + '/' + str(t[4]) + 'm' + rounds):>12}{t[10]:>13}k{t[11]:>9.2f}"
                                   f"{t[12]:>15}k{t[13]:>10.2f}{t[11] + t[13]:>10.2f}", curses.A_DIM)
                    y += 1
        y += 1
        self.put(y, 1, f"{'voyage':<44}{'':>12}{tot[0]:>13}k{tot[1]:>9.2f}{tot[2]:>15}k{tot[3]:>10.2f}{tot[1] + tot[3]:>10.2f}",
                 curses.A_BOLD)
        y += 2
        self.put(y, 1, "tokens = fresh input + output; cache reads are counted apart (prices from [prices]).", curses.A_DIM)

    def draw(self):
        self.scr.erase()
        h, w = self.scr.getmaxyx()
        top = self.header(w)
        [self.now, self.graph, self.workload, self.decisions, self.tokens][self.view](top, w, h)
        self.put(h - 1, 1, "1-5 / Tab views · [ ] voyage · ↑↓ select · Enter open · q quit · "
                           f"terminal {w}×{h}: the layout follows the width", curses.A_DIM)
        self.scr.refresh()

    def run(self):
        while True:
            self.draw()
            k = self.scr.getch()
            if k in (ord("q"), 27):
                return
            if k in (ord("\t"), curses.KEY_RIGHT):
                self.view = (self.view + 1) % len(VIEWS)
            elif k == curses.KEY_LEFT:
                self.view = (self.view - 1) % len(VIEWS)
            elif ord("1") <= k <= ord("5"):
                self.view = k - ord("1")
            elif k == ord("["):
                self.vi = min(len(VOYAGES) - 1, self.vi + 1)
                self.sel = 0
            elif k == ord("]"):
                self.vi = max(0, self.vi - 1)
                self.sel = 0
            elif k == curses.KEY_DOWN:
                self.sel = min(len(LANES[self.v["id"]]) - 1, self.sel + 1)
            elif k == curses.KEY_UP:
                self.sel = max(0, self.sel - 1)
            elif k in (10, 13, curses.KEY_ENTER):
                lane = LANES[self.v["id"]][self.sel]
                self.open ^= {lane}


if __name__ == "__main__":
    curses.wrapper(lambda scr: Board(scr).run())
