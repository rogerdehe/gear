# Stopgaps

| stopgap | why | removed when |
|---|---|---|
| gh call log | ~/.local/bin/gh wrapper logs every gh call to /Volumes/Data/tmp/roger/gh-calls.log | 2026-11-04 | remove once the GitHub API consumers are known and capped |
| merge order by hand | 2026-11-05: O-7496 (Prometheus T-738, the fix every red round waited for) was last of eight `now` merges; its `created` was set to the head of the queue in the store by hand | removed when T-574 gives the captain a plan line that puts a merge first |
| unknown merges reset by hand | 2026-11-05: Prometheus O-7384..O-7390 were left `unknown` by a batch requeue ("gated again in a smaller batch"), never retried, refused by remerge; set to `intended` in the store by hand | removed when T-576 makes a batch requeue leave its ops intended |
