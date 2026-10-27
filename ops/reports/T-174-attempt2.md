# T-174 attempt 2

F1: apply processes rebind Staff items before loading the existing-task map and queue config snapshot. An idle rebind and a NEW task in the same plan now give the new task the destination repo, base and gate, while pre-existing legacy tasks retain the old target.

F2: rebind validation projects every other pending lane's repo and ownership into the configuration before checking a new request. Conflicts are rejected atomically, including requests in the same apply; a valid earlier request from a separate apply remains able to reconcile once its attempt ends.

Regression tests cover the combined rebind and NEW plan, conflicting pending requests within one apply and across applies, and subsequent successful reconciliation. `scripts/test.sh` passed: 1,279 tests, zero failures (207 s build, 130 s suite under shared build contention).
