<!-- gear-plan project=gear base=65 -->
# Decisions
- 2026-10-23 USER corrects v65: every task already in M1 is finished — no reviews (temps = 0), the captain merges each hand-in after the build+test gate; only the review round-trips stop. M1 back: T-12, T-31, T-32, T-33. M2 stays frozen until v0.1.0 ships.

# Queue
## T-12 [must] lane=runtime milestone=M1 required
deps: -

## T-33 [must] lane=data milestone=M1 required
deps: -

## T-31 [must] lane=data milestone=M1 required
deps: -

## T-32 [must] lane=board milestone=M1 required
deps: T-31, T-26

