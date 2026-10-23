<!-- gear-plan project=gear base=3 -->
# Decisions
- 2026-10-22 T-3 review (Codex gpt-6.1-sol, ops/reviews/T-3.md on review/t3): fail on P1 raw-mode newline. Captain: the board is the one `gear` call that does not start the engine — it is read-only and must be able to show "engine down" (PRD §16 note).

# Verdicts
- T-3 attempt=1 contract=1 candidate=65c9a4fa3b3671668ecc23afad944d22925c5794 fail rework="Review ops/reviews/T-3.md (git show origin/review/t3:ops/reviews/T-3.md). P1: in raw mode (cfmakeraw clears OPOST) the frame's bare \n does not return to column 0, so rows drift right and wrap — crates/gear-board/src/screen.rs:90,104. Translate frame newlines to \r\n (or keep OPOST), and add a test that runs the real raw-mode path in a PTY and checks every row starts at column 0. P3 (fix if trivial): '1 items' -> '1 item'. Keep the board not starting the engine (captain decision)."
