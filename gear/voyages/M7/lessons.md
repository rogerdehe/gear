# M7 lessons

- **A packet names only test patterns that exist (F-346, 2026-11-06):** T-640 pointed at "the existing request-capture test pattern" for the real Herdr client; no test in the tree starts a Herdr socket, so the lane searched, asked, and built a fake socket server. Before naming a pattern, find one test that uses it and name that test.
- **A packet that adds a trait method names every implementor (T-640, 2026-11-06):** `HerdrAct::workspace_close` compiled in gear-herdr, but `crates/gear-engine/examples/opencode_live.rs` implements HerdrAct too, and the merge gate (which builds examples) failed. Before writing such a packet, `grep -rn "impl .*<Trait> for"` across the workspace and list each file in the contract.
