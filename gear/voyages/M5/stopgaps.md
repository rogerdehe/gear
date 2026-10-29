# M5 stopgaps

Workarounds added or removed during M5. Every stopgap has a removal trigger; one still open at the landing moves to the next voyage's backlog.

- tmp sweeper (2026-10-29, open): the system disk filled with lanes' and tests' temp files; a captain script outside the repo deletes, every 5 minutes, our test temp directories older than an hour and the user's /tmp entries untouched for two hours, and notifies below 10 GB free. Remove once T-412 and T-413 run in the engine and the system temp directories stop growing.
- codex hook on the old home (2026-10-29, open): /Volumes/Data/codex-home/hooks.json still runs ~/.codex/herdr-agent-state.sh, trusted at its new path as is after CODEX_HOME moved to the data disk. Remove when: `herdr integration install codex` runs with CODEX_HOME set and the new hook is trusted once; then the old ~/.codex is deleted.
