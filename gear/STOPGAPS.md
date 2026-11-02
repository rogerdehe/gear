# Stopgaps

| stopgap | why | removed when |
|---|---|---|
| `tools/build-slot/build-slot.sh` runs heavy commands over `ssh localhost` (2026-11-02) | under Herdr every new executable is its own responsible process, so macOS's Developer Tools exemption never applies and each test binary waits 40-80 s for its first-launch scan; the gate took ~20 min | a fresh binary started in a Herdr pane logs no "Doing an XProtect scan b/c not exempt" (Herdr or macOS fixed), then delete `via_ssh` |
