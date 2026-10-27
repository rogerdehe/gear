# T-160 attempt 3: overrun rework

- F1: quota parking now opens a quota pause at the seat's Parked transition and closes it at the resume transition. The existing union of pause intervals excludes this time from the round clock and no-answer timer. A fake-clock test changes the actual seat state and checks both parked and resumed intervals.
- F2: pending overrun answers are consumed before finished-round cleanup, restricted to the round's start and stop. Tests cover answers followed by development hand-in or review verdict between ticks, plus a post-hand-in answer that must not be attributed to the round.
- F3: reviewer progress for the same task, attempt, and seat resets the nudge count and cooldown, including progress from an earlier generation of the same reviewer. Reviewer-exhaustion flags are revalidated after overrun processing; independently raised missing-agent and delivery-failure flags remain. A complete fake-Herdr tick checks the captain item, absence of a stale stall flag, a fresh nudge allowance, and preservation of a subsequent missing-agent alert.

The fixes were separately committed and pushed as 4ee320e, 6ca6a17, and 7eac055; 862ee97 adds the prior-generation regression. The tested integration base is origin/dev 84d9251. The first workspace run reported 1,146 passed but its wrapper used zsh's reserved `status` variable after the suite; the corrected run is recorded below.

The corrected `scripts/test.sh` run passed all 1,146 tests (0 failures or skips). It built in 3 seconds and ran in 64 seconds wall time; eight existing tests exceeded the 10-second warning threshold under shared load. The prior clean build took 275 seconds because concurrent work contended for build resources.
