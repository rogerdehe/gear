# T-167 attempt 2: historical readers and switched-model prices

Round counting resolves the current seat profile only if its effective CLI kind matches the round's recorded CLI. Otherwise it uses the profile and project override for the recorded CLI, so later seat changes cannot select a different log reader. A matching custom profile, including one defined only in project configuration, retains its usage-log override.

Bare record model IDs now resolve to a provider-qualified configured price when exactly one provider matches. A matching round model still supplies its known provider; ambiguous switched-model matches leave cost unknown. The Codex fixtures cover a closed 300-token round counted after a switch to pi, a custom matching profile, and an m1-to-m2 switch with both unique and ambiguous price entries.

Both focused regressions passed. `scripts/test.sh` passed all 1,188 tests (107 s build, 64 s suite), with slow-test warnings under shared load.
