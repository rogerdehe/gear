# Stopgaps

| stopgap | why | removed when |
|---|---|---|
| The GEAR gate and the orch engine run inside a separate Herdr session `gatebox`, started through a disclaim launcher (`claim`, scratchpad) so Herdr is the responsible process | the orch, selyra and goldmine Herdr servers were started over SSH, so macOS has no responsible process to match against Developer Tools and scans every freshly built test binary (117-355 s per gate) | the three Herdr servers are restarted through the launcher; then stop `gatebox` and start the engines from their own sessions again |
