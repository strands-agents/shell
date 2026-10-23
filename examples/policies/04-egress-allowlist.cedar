// Example 4 — Egress allowlist (cut off the exfiltration leg).
//
// Prompt injection turns into data theft when an agent that has seen private
// data can also reach the open network: it gets tricked into POSTing your
// secrets to an attacker, or smuggling them out inside a constructed URL.
// (Simon Willison calls private data + untrusted input + external comms the
// "lethal trifecta" — https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/.)
// The robust fix is architectural: deny the network by default and allow only
// the exact egress the task needs, rather than trying to detect bad requests.
//
// This policy permits the filesystem and allows network access ONLY for GET
// requests to the documentation host. Two exfiltration vectors are closed:
//   - a POST (or PUT/DELETE) to the allowed host is denied — no upload channel;
//   - a GET to any OTHER host is denied — no constructed-URL smuggling.
// This is the policy layer; the built-in SSRF guard still blocks private,
// loopback, and IMDS addresses underneath, so a policy can only tighten egress.
//
// Run it:
//   strands-shell --policy examples/policies/04-egress-allowlist.cedar \
//     -c 'curl -sI https://docs.python.org/3/'                              # allowed
//   strands-shell --policy examples/policies/04-egress-allowlist.cedar \
//     -c 'curl -s https://example.com/'                                     # denied (host)
//   strands-shell --policy examples/policies/04-egress-allowlist.cedar \
//     -c 'curl -s -X POST -d @secrets https://docs.python.org/'             # denied (method)

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

permit(
  principal,
  action == Agent::Action::"net:request",
  resource
)
when {
  context.input.method == "GET" &&
  context.input.url like "https://docs.python.org/*"
};
