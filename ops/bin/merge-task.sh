#!/usr/bin/env bash
# merge-task.sh <n> <task-branch> [--unignore] — captain step 4b "pass", as one
# checked operation (lesson L-34). On dev in the main checkout:
#   fetch → merge origin/<branch> and origin/review/t<n> (--no-ff) → optionally
#   drop `#[ignore = "T-<n>: …"]` lines → build (zero warnings) + workspace tests
#   → push dev → delete both branches remotely and locally.
# Any failure stops before anything is pushed; a conflicted merge is aborted.
# Board/log edits stay with the captain (single writer, separate commit).
set -euo pipefail
n=${1:?task number}; branch=${2:?task branch}; unignore=${3:-}
main=$(git -C "$(dirname "$0")" worktree list --porcelain | sed -n '1s/^worktree //p')
cd "$main"
[ "$(git branch --show-current)" = dev ] || { echo "not on dev" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "dev has uncommitted changes; commit or clean first" >&2; exit 1; }
git fetch -q origin
git merge -q --ff-only origin/dev
for b in "origin/$branch" "origin/review/t$n"; do
  if ! git merge -q --no-ff "$b" -m "merge(T-$n): $b"; then
    git merge --abort; echo "conflict merging $b — send it back for rework (merge dev in), nothing pushed" >&2; exit 2
  fi
done
if [ "$unignore" = --unignore ]; then
  grep -rl "#\[ignore = \"T-$n:" crates | while read -r f; do sed -i '' "/#\[ignore = \"T-$n:/d" "$f"; done
  if [ -n "$(git status --porcelain)" ]; then git commit -qam "test: un-ignore T-$n contract tests at merge"; fi
fi
if grep -rn '^<<<<<<<\|^>>>>>>>' crates ops docs >/dev/null; then echo "conflict markers present" >&2; exit 3; fi
if cargo build --workspace --all-targets 2>&1 | grep -qE '^(warning|error)'; then echo "build not clean" >&2; exit 4; fi
cargo test --workspace >/tmp/gear-merge-test.$$ 2>&1 || { grep -E 'FAILED|panicked' /tmp/gear-merge-test.$$ | head; echo "tests fail" >&2; exit 5; }
git push -q origin dev
git push -q origin --delete "$branch" "review/t$n" || true
git branch -D "$branch" >/dev/null 2>&1 || true
echo "merged T-$n ($branch) into dev and pushed"
