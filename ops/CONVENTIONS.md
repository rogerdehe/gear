# Working conventions

GEAR runs this repository's task, review and merge workflow. Start with `gear`,
read the assigned packet with `gear --show T-<n>`, and report through `gear send`.
Use the exact result command in the packet, then end the turn.

The shared rules are [default working conventions](../skills/gear/references/conventions.md),
also available as `gear --skill conventions`. Review guidance is `gear --skill review`;
[ops/REVIEW.md](REVIEW.md) is retained as this repository's review record format.

## The last step of every voyage

Every voyage plan of GEAR ends with this task, planned at kickoff: read the voyage's lessons (`ops/lessons.md`, the voyage file's Decisions, what went to the backlog) against the shipped skill (`skills/gear/`); fix what contradicts, is stale or no longer matches the code by rewriting or deleting — never by piling on rules; then release. Nothing found means nothing to change.
