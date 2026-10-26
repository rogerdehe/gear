#!/bin/sh
# The offline proof (T-111): run the whole suite with outbound network denied.
#
# Every test is offline by construction — `.cargo/config.toml` names no `gh` and
# allows git's file transport only — so this should never be needed. It is the
# proof, and it fails when a test reaches out, so a developer in a restricted
# network can trust `cargo test` itself.
#
# macOS: sandbox-exec (unix sockets and localhost stay open, everything else is
# denied). Linux: `unshare -rn`, when user namespaces are available. Exit code
# is the suite's.
set -eu
cd "$(dirname "$0")/.."

# Build first, outside the sandbox: the first build may fetch crates. The run
# itself is `--offline`, so cargo sends nothing either.
cargo test --workspace --no-run >/dev/null

case "$(uname -s)" in
Darwin)
    profile=$(mktemp "${TMPDIR:-/tmp}/gear-offline.XXXXXX")
    trap 'rm -f "$profile"' EXIT
    cat >"$profile" <<'SB'
(version 1)
(allow default)
(deny network-outbound)
(allow network-outbound (remote unix-socket))
(allow network-outbound (remote ip "localhost:*"))
SB
    sandbox-exec -f "$profile" cargo test --workspace --offline
    ;;
Linux)
    if ! unshare -rn true 2>/dev/null; then
        echo "test-offline: no 'unshare -rn' (user namespaces): cannot deny the network" >&2
        exit 1
    fi
    unshare -rn cargo test --workspace --offline
    ;;
*)
    echo "test-offline: needs macOS (sandbox-exec) or Linux (unshare -rn); this is $(uname -s)" >&2
    exit 1
    ;;
esac
