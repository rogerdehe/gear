<!-- gear-plan project=gear base=18 -->
# Verdicts
- T-9 attempt=1 contract=2 candidate=923a5f006878a0f1b91d17839a834d7be6b0b65f fail rework="Review: git show origin/review/t9:ops/reviews/T-9.md. P1: a registered lane with no tasks disappears from the captain snapshot (the board model drops lanes without tasks, crates/gear-board/src/lib.rs:253; home.rs:169 renders that list). An empty lane is exactly what the captain must see to refill it (never-stop). Keep every registered lane in the shared model (show `<lane> · idle · 0 queued`), on the board too; add a captain-home test with one lane that has no tasks."

# Queue
## T-9 [should] lane=board milestone=M1
deps: -
