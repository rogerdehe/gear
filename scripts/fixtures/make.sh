#!/bin/sh
# make.sh — the fixture projects of the live acceptance (T-87).
#
#   sh scripts/fixtures/make.sh <dir>
#
# Builds, from nothing and offline, two projects for one small voyage each:
#
#   <dir>/origins/{parent,web,api}.git  local bare repositories: every remote a
#                                       fixture has is one of these paths
#   <dir>/multi/                        a parent repo (branch dev) with two
#                                       child repos cloned into gitignored
#                                       paths (web/, api/, branch feat), each
#                                       with its own gate
#   <dir>/running/                      a project that already has its own ways
#                                       (CI, CLAUDE.md, AGENTS.md that disagree,
#                                       .claude/skills, stale docs, one bad
#                                       pattern next to one good exemplar)
#   <dir>/FIXTURE.md                    what each one is and the config to write
#
# The same <dir> can be rebuilt at any time: the two projects and the origins
# are replaced, nothing else in <dir> is touched. No network, no real remote.
set -eu

target=${1:-}
if [ -z "$target" ]; then
    echo "usage: sh scripts/fixtures/make.sh <dir>" >&2
    exit 2
fi

mkdir -p "$target"
target=$(CDPATH= cd -- "$target" && pwd)
rm -rf "$target/multi" "$target/running" "$target/origins"
mkdir -p "$target/origins"

# Git runs here as the fixture: no identity, hooks or signing from the user's
# own configuration, so a machine with a pre-commit hook still builds a
# fixture.
git_() {
    git -c user.name="fixture" -c user.email="fixture@localhost" \
        -c commit.gpgsign=false -c core.hooksPath=/dev/null "$@"
}

commit() {
    git_ -C "$1" add -A
    git_ -C "$1" commit -qm "$2"
}

# ---------------------------------------------------------------- multi
#
# A parent repo with two child repositories below it. Each of the three has its
# own bare `origin` (a path, never a URL) and its own integration branch: the
# parent `dev`, the children `feat`. Each gate is a shell script that runs in
# well under a second.

for name in parent web api; do
    git_ init -q --bare -b dev "$target/origins/$name.git"
done
# The children's integration branch is their own (T-53): their origins start on
# `feat`, so a clone of one is on it.
for name in web api; do
    git_ -C "$target/origins/$name.git" symbolic-ref HEAD refs/heads/feat
done

# A clone of an empty bare repository warns; the next commit is the real test.
git_ clone -q "$target/origins/parent.git" "$target/multi" 2>/dev/null
parent="$target/multi"

cat > "$parent/README.md" <<'EOF'
# fixture: multi

The parent of a project with two child repositories, for GEAR's multi-repo run
(T-53). `web/` and `api/` are separate repositories, gitignored here; each has
its own origin (a local bare repository) and its own integration branch `feat`.
This repository's integration branch is `dev`.

Check it: `sh gate.sh`.
EOF

cat > "$parent/.gitignore" <<'EOF'
# The child repositories of this project live here and are their own repos.
web/
api/
EOF

cat > "$parent/main.sh" <<'EOF'
#!/bin/sh
# The parent's program: one octet, so the gate has something to run.
echo octet
EOF

cat > "$parent/gate.sh" <<'EOF'
#!/bin/sh
# The parent's gate: the project's own check before every merge. Fast, offline.
set -eu
cd "$(dirname "$0")"
test -f README.md
grep -q "fixture: multi" README.md
[ "$(sh main.sh)" = "octet" ]
EOF
chmod +x "$parent/gate.sh" "$parent/main.sh"

git_ -C "$parent" switch -q -c dev
commit "$parent" "fixture: the multi parent (dev)"
git_ -C "$parent" push -q -u origin dev
# A release branch, so a landing at the end of a voyage has one to ask about.
git_ -C "$parent" branch main dev

# The children: cloned into the gitignored paths of the parent, on `feat`.
for name in web api; do
    git_ clone -q "$target/origins/$name.git" "$parent/$name" 2>/dev/null
    child="$parent/$name"
    cat > "$child/README.md" <<EOF
# fixture: $name

One child repository of the \`multi\` fixture: its own origin (a local bare
repository), its own integration branch \`feat\`, its own gate.

Check it: \`sh gate.sh\`.
EOF
    cat > "$child/$name.sh" <<EOF
#!/bin/sh
# The $name child's program: one word, so the gate has something to run.
echo $name
EOF
    cat > "$child/gate.sh" <<EOF
#!/bin/sh
# The $name child's gate: its own check, fast and offline.
set -eu
cd "\$(dirname "\$0")"
test -f README.md
[ "\$(sh $name.sh)" = "$name" ]
EOF
    chmod +x "$child/gate.sh" "$child/$name.sh"
    git_ -C "$child" switch -q -c feat
    commit "$child" "fixture: the $name child (feat)"
    git_ -C "$child" push -q -u origin feat
    git_ -C "$child" branch main feat
done

# ---------------------------------------------------------------- running
#
# A project that already runs: its own CI, two instruction files that
# contradict each other and GEAR, an auto-loaded skill directory, a pile of
# stale docs, and one bad pattern next to one good exemplar. This is what
# `gear --skill adopt` is for (T-85).

running="$target/running"
git_ init -q -b main "$running"

mkdir -p "$running/.github/workflows" "$running/.claude/skills/release-check" \
    "$running/docs" "$running/src"

cat > "$running/README.md" <<'EOF'
# running

A project that has been running for a while: its own CI, its own instructions
for agents, its own release ritual, and a lot of history. Nothing here is
GEAR's; it is the shape `gear --skill adopt` has to meet.

Check it: `sh gate.sh` (or `make check`, which runs the same).
EOF

cat > "$running/.github/workflows/ci.yml" <<'EOF'
name: ci
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: make check
EOF

cat > "$running/CLAUDE.md" <<'EOF'
# How we work here (instructions for the agent)

- Commit straight to `main`; branches are overhead for a team this size.
- Rebase onto `main` before every push, and force-push your branch when it
  drifts. A tidy history is worth more than a shared one.
- Small changes need no tests. Ship it and watch the logs.
- Never touch `src/bad.py`: it is load-bearing (nobody remembers why).
EOF

cat > "$running/AGENTS.md" <<'EOF'
# Agent notes (keep in sync with CLAUDE.md — mostly)

- Work on a branch, one change per branch; squash and force-push before review.
- Every change needs a test, even a small one.
- `main` is protected: open a pull request and wait for one approval.
- The release ritual is manual: tag on the first Tuesday, then update
  `docs/OLD-PLAN.md` by hand.
EOF

cat > "$running/.claude/skills/release-check/SKILL.md" <<'EOF'
---
name: release-check
description: The release ritual of this project (manual, from the old days).
---

1. Tag on the first Tuesday of the month.
2. Update `docs/OLD-PLAN.md` by hand.
3. Tell the #releases channel.
EOF

cat > "$running/docs/OLD-PLAN.md" <<'EOF'
# Plan (2019, superseded)

- Move the API to SOAP by Q3.
- Hire two more backend engineers.
- Drop the CLI in favour of a web form.

Status: abandoned in 2020; nobody has read this file since.
EOF

cat > "$running/docs/architecture-2019.md" <<'EOF'
# Architecture (2019)

One process, one database, one queue on the same host. We may split the queue
later. (We never did; the queue moved to the database in 2022 and this file was
not updated.)
EOF

cat > "$running/docs/notes.md" <<'EOF'
# Notes

- 2019-04: first release.
- 2020-11: the SOAP plan was dropped.
- 2022-06: the queue moved into the database.
- TODO: delete these notes once the docs are rewritten.
EOF

cat > "$running/src/good.py" <<'EOF'
"""The exemplar: what new code here should look like."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Line:
    name: str
    price: int
    quantity: int

    def total(self) -> int:
        return self.price * self.quantity


def invoice_total(lines: list[Line]) -> int:
    """The sum of the lines; no line is special."""
    return sum(line.total() for line in lines)
EOF

cat > "$running/src/bad.py" <<'EOF'
"""The pattern new code must not copy: one file, three mistakes."""

TOTAL = 0


def add(line, seen={}):          # a mutable default argument, shared by calls
    global TOTAL                 # global state instead of a return value
    try:
        TOTAL += line["price"] * line["quantity"]
        seen[line["name"]] = seen.get(line["name"], 0) + 1
    except:                      # a bare except hides every failure
        pass
    return TOTAL
EOF

cat > "$running/gate.sh" <<'EOF'
#!/bin/sh
# This project's own gate: what CI runs before a merge. Fast, offline.
set -eu
cd "$(dirname "$0")"
test -f README.md
test -f src/good.py
grep -q "def invoice_total" src/good.py
# The bad pattern is deliberately still here; the gate does not check style.
test -f src/bad.py
EOF
chmod +x "$running/gate.sh"

cat > "$running/Makefile" <<'EOF'
# This project's own way in: `make check` is what CI runs.
.PHONY: check
check:
	sh gate.sh
EOF

commit "$running" "running: the project as it is today"

# ---------------------------------------------------------------- notes

cat > "$target/FIXTURE.md" <<'EOF'
# Fixture projects (T-87)

Built by `sh scripts/fixtures/make.sh <dir>`, offline, from nothing. Rebuild
any time: the two projects and `origins/` are replaced.

## multi/

A parent repository on `dev` with two child repositories cloned into gitignored
paths (`web/`, `api/`), each on `feat` with its own local bare `origin` under
`origins/`. Each repo has a `gate.sh` that runs in well under a second.

The config a captain writes for it (the kickoff sheet is the captain's):

```toml
[project]
id = "multi"
herdr_session = "<your Herdr session>"
repo = "<dir>/multi"

[[repos]]
name = "web"
path = "web"
integration = "feat"
gate = "sh gate.sh"

[[repos]]
name = "api"
path = "api"
integration = "feat"
gate = "sh gate.sh"

[conventions]
integration = "dev"
release = "main"
gate = "sh gate.sh"
```

## running/

A project that already runs: `.github/workflows/ci.yml`, a `CLAUDE.md` and an
`AGENTS.md` that contradict each other and GEAR, `.claude/skills/release-check`,
three stale docs under `docs/`, and `src/bad.py` beside the exemplar
`src/good.py`. Its gate is `sh gate.sh` (the same thing `make check` runs).

Use it for the adoption voyage: `gear --skill adopt` (T-85) before the kickoff.

## No network

Every remote is a path under `origins/`; nothing clones, fetches or pushes
anywhere else.
EOF

echo "fixtures: $target/multi and $target/running"
