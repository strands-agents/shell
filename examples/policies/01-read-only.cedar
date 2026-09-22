// Example 1 — Read-only sandbox.
//
// The agent may inspect the filesystem but cannot change it, reach the
// network, read the environment, or call MCP tools. This is the simplest
// useful policy: a single `permit` that names the read-only actions and
// nothing else. Every other action falls through to Cedar's default-deny.
//
// Run it:
//   strands-shell --policy examples/policies/01-read-only.cedar -c 'ls /'      # allowed
//   strands-shell --policy examples/policies/01-read-only.cedar -c 'echo hi > /home/lash/x'  # denied

permit(
  principal,
  action in [
    Agent::Action::"fs:read",
    Agent::Action::"fs:stat",
    Agent::Action::"fs:list"
  ],
  resource
);
