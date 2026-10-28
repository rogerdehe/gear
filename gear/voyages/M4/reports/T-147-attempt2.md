# T-147 attempt 2: Delivered hand-ins and project timing

A result from the owning seat and generation can move a Delivered attempt through Started to Submitted in one transaction. Other seats and stale Delivered generations are refused, and a rejected hand-in leaves the attempt unchanged. Hand-in refreshes the recorded attempt session and CLI from the current binding, clearing a stale session when the binding has none. A submitted result also confirms its pending delivery without requiring a separate check-in view.

Project config now accepts provider/model price tables and `timing.delivery_grace_s` (default 180). Validation names invalid rates and grace settings, and each engine project uses its own configured grace for delivery confirmation.

Focused tests covered ownership, generation, rollback, current and unknown session IDs, project-specific grace, and pending/late delivery evidence. `scripts/test.sh` passed all 1,211 tests (50 s build, 66 s suite). Nextest reported one leaky existing test, `waits_contract::pending_overrun_does_not_escalate_while_task_is_waiting`; existing Git-heavy fixtures also produced slow-test warnings.
