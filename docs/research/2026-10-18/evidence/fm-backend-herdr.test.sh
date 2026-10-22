ok - fm_backend_herdr_version_check: accepts the current protocol (14)
ok - fm_backend_herdr_version_check: refuses an old protocol loudly
ok - fm_backend_herdr_version_check: refuses loudly when herdr is not installed
ok - fm_backend_herdr_workspace_label: a primary home (no marker) resolves to 'firstmate'
ok - fm_backend_herdr_workspace_label: a secondmate home (.fm-secondmate-home) resolves to '2ndmate-<id>'
ok - fm_backend_herdr_workspace_label: trims whitespace around the marker's secondmate id
ok - fm_backend_herdr_workspace_label: an empty marker file falls back to the primary label 'firstmate'
ok - fm_backend_herdr_workspace_label: two different secondmate homes get two different, non-colliding labels
ok - fm_backend_herdr_cli: sets HERDR_SESSION AND appends a trailing --session flag on every call
ok - herdr client selection: a live pane behind a stale shadowing client reads alive
ok - herdr recovery-grade read: a stopped server means missing there, and nowhere else
ok - herdr stale registration: a shell-only pane with a lingering Pi record is agent-free with an explicit reason
ok - herdr stale registration: no registered status can outrank a shell-only process view
ok - herdr pane agent session: a resumable reference is reported with the agent label that reported it
ok - herdr pane agent session: anything unresumable degrades to a nonzero read with no output
ok - herdr stale registration: a registered agent with a live Pi foreground process still reads alive
ok - herdr stale registration: only a shell-only pane demotes a registration
ok - herdr stale registration: a transient prompt helper settles into stale-agent instead of reading live
ok - herdr stale registration: an exhausted settle window still reads a non-shell foreground as live
ok - herdr stale registration: an agent process outside the foreground group still counts as alive
ok - herdr stale registration: the descendant walk reads a spaced executable path whole
ok - herdr stale registration: an unreadable process view refuses instead of guessing either way
ok - herdr stale registration: an empty foreground list over a real shell is not unreadable, it settles via the descendant walk
ok - herdr stale registration: presentation reclaim never closes a stale-registration pane
ok - herdr stale registration: busy_state proves a working record at process level before reporting busy
ok - herdr client selection: one selected client is reused per process
ok - herdr client selection: selected clients remain scoped to their session
ok - herdr client selection: only protocol_mismatch triggers reselection; other failures pass through untouched
ok - herdr client selection: the happy path makes no extra call
ok - herdr client status: .server.compatible, legacy protocol equality, unknown, and stopped shapes all normalize
ok - fm_backend_herdr_launcher_identity: a firstmate not running inside herdr has no launcher workspace to inherit
ok - fm_backend_herdr_launcher_identity: HERDR_ENV=1 without a pane id selects the backend but binds no parent
ok - fm_backend_herdr_launcher_identity: resolves the launcher's exact workspace even when a same-labeled workspace sorts first
ok - fm_backend_herdr_launcher_identity: refuses a launcher pane that names a different herdr session
ok - fm_backend_herdr_launcher_identity: refuses a claimed pane without exact server identity
ok - fm_backend_herdr_launcher_identity: refuses a launcher pane whose injected socket belongs to another herdr server
ok - fm_backend_herdr_launcher_identity: refuses when the launcher's own pane no longer resolves
ok - fm_backend_herdr_launcher_identity: refuses when the launcher's pane and tab disagree about their workspace
ok - fm_backend_herdr_launcher_identity: refuses when the launcher's workspace is gone from its own session
ok - fm_backend_herdr_workspace_ensure: places a worker in the launcher's exact workspace, not the first same-labeled one
ok - fm_backend_herdr_workspace_ensure: refuses to guess between two same-labeled home workspaces
ok - fm_backend_herdr_workspace_ensure: a --secondmate container resolves that home's own workspace, not the launcher's
ok - fm_backend_herdr_container_ensure: surfaces the exact ambiguous-placement refusal instead of a generic failure
ok - fm_backend_herdr_container_ensure: version-gates, starts the server, ensures the firstmate workspace, echoes session:workspace_id + the seeded default tab id
ok - fm_backend_herdr_server_ensure: scrubs home and harness identity without disturbing unrelated environment or session routing
ok - fm_backend_herdr_container_ensure: reuses an existing firstmate workspace without recreating it, and reports no seeded default tab (adopted, not created)
ok - fm_backend_herdr_container_ensure: workspace create passes --no-focus
ok - fm_backend_herdr_container_ensure: creates the workspace under the SECONDMATE home's own label, not 'firstmate'
ok - fm_backend_herdr_create_task: prunes exactly the seeded default tab container_ensure identified, once the first real task tab exists
