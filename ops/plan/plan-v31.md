<!-- gear-plan project=gear base=30 -->
# Decisions
- 2026-10-23 USER (clarifies v30): "freeze" means M1 takes no new scope; M2 is not paused. M1 first; a lane with no M1 work takes M2 work (never-stop). M2 tasks are dispatchable again, ranked after each lane's M1 tasks.

# Queue
## T-13 [must] lane=data milestone=M1 required
deps: -

## T-17 [must] lane=runtime milestone=M1 required
deps: -

## T-12 [must] lane=runtime milestone=M1 required
deps: -

## T-18 [must] lane=runtime milestone=M1 required
deps: T-17

## T-15 [must] lane=board milestone=M1 required
deps: -

## T-7 [should] lane=board milestone=M2
deps: -

## T-16 [should] lane=board milestone=M2
deps: -

## T-8 [should] lane=data milestone=M2
deps: -
