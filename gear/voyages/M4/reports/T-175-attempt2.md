# T-175 attempt 2: rebind recovery and retained checkouts

A reserved or uncertain exit now has a bounded wait. If the old agent remains, the lane is parked in NeedsCheck with an actionable captain retry; retry preserves the rebind barrier and does not blindly repeat an uncertain exit. The old repository is captured in the applied marker, and checkout paths are retained by seat and repository so normal onboarding can reopen a previously visited checkout.

Tests cover uncertain exit and a restart after reservation through captain retry, plus A → B → A checkout reopening and a fresh next session. `scripts/test.sh` passed all 1,299 tests (build 347 seconds, suite 118 seconds). The previously reported slow real-Git merge fixture remains unchanged.
