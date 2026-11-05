# Stopgaps

| stopgap | why | removed when |
|---|---|---|
| gh call log | ~/.local/bin/gh wrapper logs every gh call to /Volumes/Data/tmp/roger/gh-calls.log | 2026-11-04 | remove once the GitHub API consumers are known and capped |
| `target/` in the repo's shared `info/exclude` | branches cut before 63451561 (gear/feature/f-1, f-2 and their review branches) still ignore only `/target`, so reviewer worktrees stash test temp dirs and ring | remove once F-1 and F-2 have landed on dev |
