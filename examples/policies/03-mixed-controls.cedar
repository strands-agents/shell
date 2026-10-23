// Example 3 — Mixed controls with a forbid override.
//
// A more realistic policy that combines several rules:
//   - read/stat/list anywhere;
//   - write/create/delete only under the home directory;
//   - but NEVER read files ending in `.secret` (a `forbid` that overrides the
//     broad read permit — in Cedar, any matching `forbid` wins over every
//     `permit`);
//   - GET requests to `https://example.com/...` only — all other network
//     access (other hosts, other methods) is denied;
//   - environment enumeration (`env`) and MCP tool calls are denied, since no
//     clause permits `env:read` or `mcp:call`.
//
// This demonstrates layering permits, scoping by action and by input field,
// and using `forbid` to carve an exception out of a broad allow.
//
// Run it:
//   strands-shell --policy examples/policies/03-mixed-controls.cedar \
//     -c 'echo hi > /home/lash/n.txt; cat /home/lash/n.txt'        # allowed
//   strands-shell --policy examples/policies/03-mixed-controls.cedar \
//     -c 'echo s > /home/lash/k.secret; cat /home/lash/k.secret'   # write ok, read denied
//   strands-shell --policy examples/policies/03-mixed-controls.cedar \
//     -c 'env'                                                      # denied

// Read the filesystem freely...
permit(
  principal,
  action in [
    Agent::Action::"fs:read",
    Agent::Action::"fs:stat",
    Agent::Action::"fs:list"
  ],
  resource
);

// ...write only under the home directory...
permit(
  principal,
  action in [
    Agent::Action::"fs:write",
    Agent::Action::"fs:create",
    Agent::Action::"fs:delete"
  ],
  resource
)
when { context.input.path like "/home/lash/*" };

// ...but secrets are never readable, even though the broad read permit above
// would otherwise allow it. A matching forbid always wins.
forbid(
  principal,
  action in [
    Agent::Action::"fs:read",
    Agent::Action::"fs:stat"
  ],
  resource
)
when { context.input.path like "*.secret" };

// Network: GET to example.com only.
permit(
  principal,
  action == Agent::Action::"net:request",
  resource
)
when {
  context.input.url like "https://example.com/*" &&
  context.input.method == "GET"
};
