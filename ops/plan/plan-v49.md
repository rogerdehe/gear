<!-- gear-plan project=gear base=48 -->
# Queue
## T-26 [must] lane=board milestone=M1 required
goal: Finish the user-facing README.md the captain rewrote (read it first): a new user goes from nothing to a running project. HARD RULE (user): nothing in the user's path mentions cargo, Rust or building — installing is `herdr plugin install` plus the prebuilt binary (T-27); building from source appears only under "Developing GEAR". Verify every user command on this machine where it can be (mark each block verified / not verifiable here), fix the PATH line to the real install location Herdr uses for a GitHub plugin (`herdr plugin list` shows it), check `gear admin kickoff` output really is a usable gear.toml draft, and say exactly how to install the skill for Claude Code / OpenCode / Codex captains. Short and true.
accept: README.md reads top to bottom as the quickstart; no cargo/Rust outside "Developing GEAR"; every command block marked verified or not-verifiable-here with the reason; no code changes.
deps: -
