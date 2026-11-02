# Lessons

Pits and lessons of voyage M6.2, recorded as they happen; read against the shipped skill before landing.
- 2026-11-02: A scratch `gear admin init` (GEAR_DB on a throwaway store, to check a captain-only config) run inside a Herdr pane re-registered that session's store in ~/.config/gear/targets.tsv; at the next restart GEAR's own engine refused to start ("not this Herdr session's GEAR store") until the line was put back. Try a config against a scratch store only outside Herdr (no HERDR_SOCKET_PATH), and check `pgrep -fl 'gear run'` shows every engine after a restart.
