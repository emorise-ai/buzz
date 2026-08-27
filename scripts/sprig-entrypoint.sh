#!/bin/bash
set -euo pipefail

# Match desktop's URL-scoped git credential configuration without installing a
# helper globally (which would make it answer for unrelated remotes).
if [[ -n "${BUZZ_RELAY_URL:-}" ]]; then
    relay_http_url="${BUZZ_RELAY_URL/#ws:/http:}"
    relay_http_url="${relay_http_url/#wss:/https:}"
    relay_http_url="${relay_http_url%/}"
    git config --global "credential.${relay_http_url}/git.helper" \
        /usr/local/bin/git-credential-nostr
    git config --global "credential.${relay_http_url}/git.useHttpPath" true
fi

# Two shapes of sandbox.
#
# By default the harness runs here and the sandbox reasons for itself: it
# connects to the relay, decides when to act, and calls a model — which means
# an LLM credential has to live in this container.
#
# With BUZZ_DEV_MCP_BIND set, the sandbox instead serves only its tools, and
# the agent's brain runs elsewhere and drives them over authenticated HTTP.
# Nothing here connects to a relay or holds a model credential; this container
# is hands, not a mind. See crates/buzz-dev-mcp/src/serve_http.rs.
#
# Either way the long-running process is exec'd, so it is PID 1 and receives
# the container's termination signal directly (docs/remote-agents.md, L1
# launcher obligations — a wrapper that swallows the signal conforms to
# nothing).
if [[ -n "${BUZZ_DEV_MCP_BIND:-}" ]]; then
    exec buzz-dev-mcp
fi

exec buzz-acp "$@"
