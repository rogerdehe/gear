#!/usr/bin/env bash
# timer.sh <minutes> <reason> — manual stand-in for PARKED-until (design/10).
# Run in the background; it exits after <minutes> and prints <reason>, which
# wakes the captain, who then checks the pane and prompts the agent (never this
# script: typing into an agent needs the occupant check, lesson L-1).
set -eu
m=${1:?minutes}; shift
sleep $((m * 60))
echo "timer: $*"
