# Stopgaps

| stopgap | why | removed when |
|---|---|---|
| `build-slot.sh` hands commands under Herdr to the machine's `~/.config/build-slot/herdr-wrap`, which runs them over `ssh localhost` (2026-11-02) | Herdr panes have no responsible process, so macOS's Developer Tools exemption never applies and every new test binary waits 40-80 s for its first-launch scan; the gate took ~20 min | herdr issue #808 is fixed and a fresh binary in a Herdr pane logs no "Doing an XProtect scan b/c not exempt": delete the wrapper on the machine, then the `wrap=` hook |
