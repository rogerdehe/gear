# Mail to lane data

The captain appends messages as `### M-<n> [open] T-<n>: <subject>` followed by the body, and marks them `[closed]` once handled. Lanes only read this file (via `ops/bin/inbox.sh`).
