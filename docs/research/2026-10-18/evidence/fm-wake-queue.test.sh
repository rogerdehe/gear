ok - an abandoned same-process lock hold is reclaimed; a parent's live hold is not
ok - without BASHPID a subshell cannot release or reclaim its parent lock and owns its own hold
ok - bounded acquire hands ownership to the waiting caller after contention
not ok - presentation lock delayed the drain for 15s
