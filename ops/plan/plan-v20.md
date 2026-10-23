<!-- gear-plan project=gear base=19 -->
# Queue
## NEW [should] lane=board milestone=M1
goal: The board shows how well never-stop is going (the two measures in docs/field-notes/decomposition.md). Per lane: rework rate = rejected attempts / handed-in attempts (e.g. `rework 1/3`), and the time since the lane last received a task (from its last Deliver op), e.g. `last task 12m ago`. One totals line for the project: overall rework rate. Read from the store only; also in `--json`. The engine runs concurrently and lanes may have no tasks at all — both must render (an empty lane shows `rework 0/0 · no task yet`).
accept: cargo test --workspace green; a contract test with one lane (2 handed in, 1 rejected, last deliver 5 min ago) and one empty lane checks the rendered lines; prove it on a scratch on-disk store with the built binary in a PTY (every row starts at column 0) and paste the frame in the report.
deps: -
