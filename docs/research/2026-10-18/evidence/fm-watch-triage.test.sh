ok - status_span_has_actionable: benign absorbed, captain events surfaced, classified events not re-fired
ok - an actionable event is not hidden by later routine appends, and is named as itself
ok - span classification retires closed decisions and surfaces rejected transitions for reconciliation
ok - span classification from an offset keeps closed decisions closed and live ones live
ok - a malformed seen signature causes the whole status log to be classified
ok - stale_is_terminal: terminal status surfaces, non-terminal and no-status are benign
ok - classifier primitives: keyed decisions and activity phases, captain relevance, window-to-task, and overrides
ok - unrecognized status prefixes are visible and recognized prefixes are unchanged
ok - crew_is_provably_working: only working+run-step/pane is provable; idle/finished/parked/failed/unknown surface
ok - status_is_paused: only the leading paused verb matches, paused is not captain-relevant, and the two declared-wait verbs stay separable
ok - crew_absorb_class: working/paused/none from one read; crew_is_paused and crew_is_provably_working agree
ok - crew_worktree_written_since: real writes are evidence; no worktree, no anchor, quiet trees, .git churn and a mate's own home are not
ok - an empty FM_WORKTREE_WRITE_PRUNE widens the probe to the whole depth-bounded tree instead of disabling it
ok - an empty FM_WORKTREE_WRITE_PRUNE exported into the environment prunes nothing, widening the probe
ok - the worktree write probe is wall-clock bounded, and hitting the bound reads as no write evidence
ok - signal_crew_provably_working: benign only when every referenced crew is provably working
ok - a secondmate's unmarked routine progress absorbs when provably working; routed, terminal, note, marked, and unknown lines surface
ok - a no-verb signal whose crew is provably working is absorbed (no exit, no queue, suppressor advanced, beacon present)
ok - a bare turn-end whose crew is provably working (busy pane) is absorbed
wait_for_exit: owned pid 55179 exceeded 100 polls; sending TERM
55179 43856 R    bash /tmp/orch-deep-research/firstmate/bin/fm-watch.sh
not ok - watcher did not surface a turn-end whose crew is not provably working
