#!/usr/bin/env python3
"""GEAR board prototype v3 for M5 (fake data, curses, no dependencies).

Keys: 0-5 or Tab / <- -> views · [ ] older / newer voyage · up/down select ·
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
VIEWS = ["Voyages", "Now", "Graph", "Workload", "Decisions", "Time & tokens"]

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
        state = "sailing" if v["state"] == "sailing" else "landed"
        self.put(1, 1, f"{v['id']} {v['title']} · {state} · bar {v['bar']} · {hm(v['used'])} of {hm(v['eta'])}"
                       f" · projected {hm(v['projected'])}", curses.A_BOLD)
        y = 2
        for line in wrap("Goal: " + v["done"], w - 4):
            self.put(y, 1, line)
            y += 1
        if v["goal_changed"]:
            at, before, after = v["goal_changed"]
            self.put(y, 1, f"goal changed {at}: {before} -> {after} (with the user's yes; Decisions shows both)",
                     curses.color_pair(10))
            y += 1
        # stations as one line across the full width
        x = 1
        self.put(y, x, "Stations ", curses.A_DIM)
        x += 9
        for sid, what, st, at in v["stations"]:
            mark = {"done": "✓", "now": "▶", "next": "·"}[st]
            a = curses.color_pair(11) if st == "done" else (curses.A_BOLD if st == "now" else curses.A_DIM)
            label = f"{mark} {sid} {what} ({hm(at)})"
            self.put(y, x, label, a)
            x += len(label) + 3
        y += 1
        self.put(y, 1, "Distance: " + " · ".join(v["distance"]), curses.A_DIM)
        y += 1
        x = 1
        for i, name in enumerate(VIEWS):
            label = f" {i} {name} "
            self.put(y, x, label, curses.A_REVERSE if i == self.view else 0)
            x += len(label) + 1
        self.put(y + 1, 0, "─" * (w - 1), curses.A_DIM)
        return y + 2

    # -- 0 Voyages: every voyage of the project, its goal and how it ended
    def voyages(self, top, w, h):
        y = top
        self.put(y, 1, "Every voyage, newest first. ↑↓ or [ ] picks one; the other views show the one picked.", curses.A_DIM)
        y += 2
        for i, v in enumerate(VOYAGES):
            a = curses.A_REVERSE if i == self.vi else 0
            state = "sailing" if v["state"] == "sailing" else "landed"
            self.put(y, 1, f"{v['id']:<6}{state:<9}{hm(v['used']):>7} of {hm(v['eta']):<7} bar {v['bar']:<12}{v['title']}",
                     curses.A_BOLD | a)
            y += 1
            for line in wrap("Goal: " + v["done"], w - 12):
                self.put(y, 8, line)
                y += 1
            if v["goal_changed"]:
                at, before, after = v["goal_changed"]
                self.put(y, 8, f"goal changed {at}: {before} -> {after}", curses.color_pair(10))
                y += 1
            for part in v["distance"]:
                bad = "NOT" in part or "not started" in part or "measured" in part
                self.put(y, 8, ("✗ " if bad else "✓ ") + part, curses.color_pair(10) if bad else curses.color_pair(11))
                y += 1
            y += 1
            if y >= h - 3:
                break

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
            self.put(y, 9, f"done {len(done)} ({hm(sum(t[5] for t in done))}) · left {len(left)} "
                           f"({hm(sum(t[4] for t in left))})", curses.A_DIM)
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

    # -- 2 Graph: dev as the main line, each task a branch from where its work is based
    def graph(self, top, w, h):
        tasks = T[self.v["id"]]
        lanes = LANES[self.v["id"]]
        events = []
        for t in tasks:
            if t[7] is not None:
                events.append((t[7], "start", t))
            if t[8] is not None:
                events.append((t[8], "merge", t))
        events.sort(key=lambda e: (e[0], e[1] == "start"), reverse=True)
        waiting = [t for t in tasks if t[7] is None and t[2] != "moved"]
        col = {lane: 6 + 6 * i for i, lane in enumerate(lanes)}
        y = top
        self.put(y, 1, "dev", curses.A_BOLD)
        for lane in lanes:
            self.put(y, col[lane] - 1, lane[:5], self.lane_attr(lane))
        self.put(y, col[lanes[-1]] + 6, "newest on top · ● merged into dev · ◉ working · ◎ in review · ↺ rework · ┆ waits",
                 curses.A_DIM)
        y += 1
        # waiting tasks hang dashed from the dependency they wait for
        for t in waiting:
            c = col[t[1]]
            self.put(y, 1, "│")
            self.put(y, c, "○", self.lane_attr(t[1]))
            self.put(y, col[lanes[-1]] + 4, f"{t[0]} {t[3]} — {words(t)}", curses.A_DIM)
            y += 1
            self.put(y, 1, "│")
            self.put(y, c, "┆", self.lane_attr(t[1]))
            y += 1
        active = {t[0] for t in tasks if t[7] is not None and t[8] is None}
        live = set(active)
        for at, kind, t in events:
            if y >= h - 2:
                break
            c = col[t[1]]
            line = [" "] * (col[lanes[-1]] + 2)
            line[1] = "│"
            for other in tasks:
                if other[0] in live and other[0] != t[0]:
                    line[col[other[1]]] = "│"
            if kind == "merge":
                line[1] = "●"
                for x in range(2, c):
                    line[x] = "─"
                line[c] = "╯"
                live.add(t[0])
                label = f"{t[0]} {t[3]} — merged at +{hm(at)}" + (f" · ↺ {t[6]} rounds" if t[6] > 1 else "")
            else:
                line[1] = "├"
                for x in range(2, c):
                    line[x] = "─"
                line[c] = "╮"
                live.discard(t[0])
                mark = {"working": "◉", "review": "◎", "rework": "↺"}.get(t[2], "")
                label = f"{t[0]} starts at +{hm(at)}" + (f" (after {', '.join(t[9])})" if t[9] else "")
                if t[0] in active:
                    label = f"{mark} {t[0]} {t[3]} — {words(t)} · {t[5]}/{t[4]}m"
            self.put(y, 0, "".join(line))
            self.put(y, c, "".join(line)[c], self.lane_attr(t[1]) | curses.A_BOLD)
            self.put(y, len(line) + 2, label, curses.A_BOLD if t[0] in active else 0)
            y += 1

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
        [self.voyages, self.now, self.graph, self.workload, self.decisions, self.tokens][self.view](top, w, h)
        self.put(h - 1, 1, "0-5 / Tab views · [ ] voyage · ↑↓ select · Enter open · q quit · "
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
            elif ord("0") <= k <= ord("5"):
                self.view = k - ord("0")
            elif k == ord("["):
                self.vi = min(len(VOYAGES) - 1, self.vi + 1)
                self.sel = 0
            elif k == ord("]"):
                self.vi = max(0, self.vi - 1)
                self.sel = 0
            elif k == curses.KEY_DOWN and self.view == 0:
                self.vi = min(len(VOYAGES) - 1, self.vi + 1)
            elif k == curses.KEY_UP and self.view == 0:
                self.vi = max(0, self.vi - 1)
            elif k == curses.KEY_DOWN:
                self.sel = min(len(LANES[self.v["id"]]) - 1, self.sel + 1)
            elif k == curses.KEY_UP:
                self.sel = max(0, self.sel - 1)
            elif k in (10, 13, curses.KEY_ENTER):
                lane = LANES[self.v["id"]][self.sel]
                self.open ^= {lane}


if __name__ == "__main__":
    curses.wrapper(lambda scr: Board(scr).run())
