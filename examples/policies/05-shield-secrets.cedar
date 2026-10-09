// Example 5 — Shield secrets from an untrusted-data agent (cut off the
// private-data leg).
//
// A coding agent often needs broad read/write over a project tree, but that
// same tree usually contains things it should never read: `.env` files, API
// keys, SSH private keys, cloud credentials. If an injected instruction can
// make the agent read those and then leak them, you've lost. Rather than
// hoping the agent "won't look," deny reads of secret-shaped paths outright —
// a `forbid` always wins over the broad read permit, so the carve-out holds no
// matter what the agent is talked into doing.
//
// The agent keeps full read/write of ordinary files (and listing a directory
// still works — it just can't read the secret's contents). Pair this with
// Example 4's egress allowlist for defense in depth: even a leaked secret has
// nowhere to go. Note there is NO `net:request` permit here, so this policy
// also denies the network entirely.
//
// Matching note: `like` is a glob over the whole path. `*.env` catches
// `config.env`; `*/.env` catches a bare `.env` in any directory; `*/.ssh/*`
// catches anything under an `.ssh` directory.
//
// Run it:
//   strands-shell --policy examples/policies/05-shield-secrets.cedar \
//     -c 'cat /home/lash/project/app.py'        # allowed
//   strands-shell --policy examples/policies/05-shield-secrets.cedar \
//     -c 'cat /home/lash/project/.env'          # denied
//   strands-shell --policy examples/policies/05-shield-secrets.cedar \
//     -c 'cat /home/lash/.ssh/id_rsa'           # denied

// Read and write the working tree freely...
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
);

// ...but never read credentials, env files, or private keys, anywhere. A
// matching forbid overrides every permit above.
forbid(
  principal,
  action in [
    Agent::Action::"fs:read",
    Agent::Action::"fs:stat"
  ],
  resource
)
when {
  context.input.path like "*.env" ||
  context.input.path like "*/.env" ||
  context.input.path like "*.pem" ||
  context.input.path like "*/.ssh/*" ||
  context.input.path like "*credentials*"
};
