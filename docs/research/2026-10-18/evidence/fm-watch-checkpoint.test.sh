ok - quiet checkpoint exits 124 with a clean checkpoint line and no live lock
ok - Perl fallback honors the cleanup bound before escalating an INT-ignoring direct watcher
ok - an announced recovery is not reannounced by the next foreground checkpoint
ok - a never-announced recovery still surfaces once from a foreground checkpoint
not ok - queue-append checkpoint did not take the watcher lock
