#!/usr/bin/env bash
# switch-model.sh <agent> <filter> <expect>
# Switch a live OpenCode agent's model with /model (SOP agent:opencode).
# <filter> is typed into the picker; <expect> must then appear on the status bar.
# Refuses while the agent is working (never type into a busy agent; lesson L-1).
set -eu
agent=${1:?agent}; filter=${2:?picker filter}; expect=${3:?status-bar text}
st=$(herdr agent get "$agent" | sed -n 's/.*"agent_status":"\([a-z]*\)".*/\1/p')
case "$st" in idle|done) ;; *) echo "$agent is $st; switch only when it has stopped" >&2; exit 1 ;; esac
bar() { herdr agent read "$agent" --source visible | grep -E 'Build auto' | tail -1; }
bar | grep -qF "$expect" && { echo "$agent already on $expect"; exit 0; }
herdr agent send-keys "$agent" esc >/dev/null; sleep 0.5      # close a quota toast / stray input
herdr pane send-text "$(herdr agent get "$agent" | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p')" "/model" >/dev/null
herdr agent send-keys "$agent" enter >/dev/null; sleep 1
herdr pane send-text "$(herdr agent get "$agent" | sed -n 's/.*"pane_id":"\([^"]*\)".*/\1/p')" "$filter" >/dev/null; sleep 1
herdr agent send-keys "$agent" enter >/dev/null; sleep 1
bar | grep -qF "$expect" || { echo "model check failed: $(bar)" >&2; exit 1; }
echo "$agent now on $expect"
