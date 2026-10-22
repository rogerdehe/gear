# Mail to lane data

The captain appends messages as `### M-<n> [open] T-<n>: <subject>` followed by the body, and marks them `[closed]` once handled. Lanes only read this file (via `ops/bin/inbox.sh`).

### M-1 [open] T-16: rework — fix the P2 and the test P3 from R-9

Read `origin/review/t16:ops/reviews/T-16.md`. Fix the P2 (`max_threads = 0` must return no thread: reserve the slot only when `max_threads > 0`) with a test, and the first P3 (`last_reply_is_newest_reply` must store replies as `MsgState::Info` so the whole-thread read is exercised); refresh the Result block. Captain answers to the reviewer's questions: `per_sender` counts a thread once, for the sender of its oldest unhandled message (your choice stands — add one sentence to the doc comment); T-17 passes `BatchLimits::default()`.
