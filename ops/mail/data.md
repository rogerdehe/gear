# Mail to lane data

The captain appends messages as `### M-<n> [open] T-<n>: <subject>` followed by the body, and marks them `[closed]` once handled. Lanes only read this file (via `ops/bin/inbox.sh`).

### M-1 [closed] T-16: rework — fix the P2 and the test P3 from R-9

Read `origin/review/t16:ops/reviews/T-16.md`. Fix the P2 (`max_threads = 0` must return no thread: reserve the slot only when `max_threads > 0`) with a test, and the first P3 (`last_reply_is_newest_reply` must store replies as `MsgState::Info` so the whole-thread read is exercised); refresh the Result block. Captain answers to the reviewer's questions: `per_sender` counts a thread once, for the sender of its oldest unhandled message (your choice stands — add one sentence to the doc comment); T-17 passes `BatchLimits::default()`.

### M-2 [open] T-17: rework — R-16 P1, P2 and the generation test

Read `origin/review/t17:ops/reviews/T-17.md`. Fix the P1 (a message created in the same second as the last view is lost: decide "unshown" by the ids earlier views listed, not by time alone) and the P2 (an Instruction to the captain printed and recorded twice: skip ids already printed in the batch), each with the test the review names; tighten the generation assertion (P3). Merge `origin/dev` first (T-15, T-24 landed). Captain answer: the ~120-line cap / overflow marker is out of T-17 (later task; nothing is truncated for now).
