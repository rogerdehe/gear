# T-198 attempt 2: direction result after submission

A bound, running direction reviewer can now return its result while the lane attempt is Started or Submitted. The existing seat, generation, job and SHA checks remain; the result leaves the attempt, task and verdict unchanged.

The CLI regression submits the lane first, then verifies the reviewer result's checked SHA and the unchanged Submitted and Verifying states. `scripts/test.sh` passed all 1,372 tests (build 302 seconds, suite 137 seconds).
