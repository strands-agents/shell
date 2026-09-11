// Example 2 — Workspace jail.
//
// The agent gets full read/write access, but only inside
// `/home/lash/workspace`. Reads and writes anywhere else are denied. This
// shows matching on per-call input with the `like` operator and scoping a
// clause to the actions that actually carry a `path` field.
//
// Note: a clause that reads `context.input.path` must apply only to actions
// whose schema has that field. `fs:rename` uses `src`/`dst` (not `path`) and
// the non-fs actions have no path at all, so they are intentionally left out
// of the action list here — including them would fail schema validation.
//
// The `== "/home/lash/workspace"` term covers the directory itself; the
// `like "/home/lash/workspace/*"` term covers everything beneath it.
//
// Run it:
//   strands-shell --policy examples/policies/02-workspace-jail.cedar \
//     -c 'mkdir /home/lash/workspace; echo hi > /home/lash/workspace/f; cat /home/lash/workspace/f'   # allowed
//   strands-shell --policy examples/policies/02-workspace-jail.cedar \
//     -c 'echo hi > /home/lash/escape'                                                                 # denied

permit(
  principal,
  action in [
    Agent::Action::"fs:read",
    Agent::Action::"fs:stat",
    Agent::Action::"fs:list",
    Agent::Action::"fs:write",
    Agent::Action::"fs:create",
    Agent::Action::"fs:delete"
  ],
  resource
)
when {
  context.input.path == "/home/lash/workspace" ||
  context.input.path like "/home/lash/workspace/*"
};
