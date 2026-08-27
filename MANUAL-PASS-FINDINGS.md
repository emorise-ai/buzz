# Manual pass — what actually happened

**Run 2026-08-16** against the live staging relay
(`wss://relay.staging.emorise.com`) and the Matterhorn broker, driving the real
provider binary. This is the record `HANDOFF-TESTING.md` asks for: what was
driven by hand, what it did, and what the automated tests must therefore assert.

Every sandbox created during this pass was removed; the host was verified clean
afterwards (only `buzz-sandbox-broker` remains running).

---

## Headline

**The provider → broker → container → relay → agent-dispatch chain works end to
end.** The one thing never proven before — an agent receiving a mention and
invoking its model — now runs. It stops at the LLM call itself, and only
because no valid API key was used:

```
discovered 1 channel(s)
subscribed to channel 9b4d34e8-…
[MCP] buzz-dev-mcp initialized, client initialized
agent_returned outcome="error"
  error=Agent reported error (code -32001):
        llm auth: 401: static key rejected — update key in agent settings
```

Everything upstream of that 401 is verified working. A valid key is the only
remaining input needed for a literal reply in a channel.

---

## Verified this pass

### 1. Provider → broker with the new signing (the highest-risk gap)

`HANDOFF-TESTING.md` flagged this as untested since the auth change and said to
test it first. **It works.**

```
$ buzz-backend-docker < deploy.json
{"ok":true,"agent_id":"51c7f71d…"}
```

The provider signs with the agent's own key (NIP-98: `u`, `method`, `payload`
tags); the broker verifies the signature and creates the sandbox. No bearer
token anywhere in the path.

### 2. Resource limits are really enforced

Read back from Docker rather than trusting the response body:

```
NanoCpus=1000000000   (1.0 CPU)
CpusetCpus=2-2        (pinned — the `nproc` gotcha is handled)
Memory=1073741824     (1 GiB)
PidsLimit=2048
CapDrop=[ALL]
```

### 3. Both lifecycle events publish

Broker logs show `published a sandbox event kind=48200` on create and
`kind=48201` on reap, accepted by the relay.

### 4. Live membership pickup

An agent that starts with no channels picks one up **without a restart** when
membership is granted:

```
discovered 0 channel(s)
WARN no channel subscriptions resolved — agent will sit idle
…later, after `channels add-member`…
membership notification: subscribing to new channel channel_id=9b4d34e8-…
```

### 5. Provider-agnostic env passthrough

A sandbox deployed with `BUZZ_AGENT_PROVIDER=openrouter` +
`OPENROUTER_API_KEY` + `BUZZ_AGENT_MODEL` carried all three into the container
and `buzz-agent` started cleanly against a non-Anthropic provider. The sandbox
does not assume Claude.

### 6. The full agent stack on the claude image

`buzz-sprig-claude` starts `claude-agent-acp`, initializes ACP, connects to the
relay, brings up Xvfb + noVNC + Chromium (`agent control on :9222`), and sets
presence online.

---

## Findings — things that are wrong or missing

### F1. Sandbox images ship only one ACP adapter *(gap, affects the product)*

The harness is adapter-generic: `BUZZ_ACP_AGENT_COMMAND` accepts any ACP
adapter, and `buzz-acp` has dedicated Codex support
(`build_codex_config_env`, `codex_network_env`). But the images do not:

```
goose             MISSING
codex             MISSING
codex-acp         MISSING
claude-agent-acp  /usr/bin/claude-agent-acp
cursor-agent      MISSING
```

`Dockerfile.sprig-claude:36` installs only
`@agentclientprotocol/claude-agent-acp` and pins
`BUZZ_ACP_AGENT_COMMAND=claude-agent-acp` at line 43.

So a sandbox today can run Claude Code (subscription) or `buzz-agent`
(pay-per-token API key), but **not Codex or Cursor**, despite the harness
supporting them. This is an image-contents gap, not an architecture flaw — the
fix is an npm install per adapter.

### F2. No credential-injection path for subscription-based agents *(gap)*

`buzz-sprig-claude` contains no `~/.claude/.credentials.json`, and
`claude auth status` inside a fresh sandbox reports:

```json
{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}
```

The provider has no mechanism to inject Claude credentials. The earlier
"verified" note in `HANDOFF.md` came from credentials copied in by hand during
an ad-hoc session. So the subscription path — the one that needs no API key —
**cannot currently be established through a deploy**. Pay-per-token providers
work because their key is just an env var.

This is the main thing standing between the current state and a hands-off
subscription agent.

### F3. Dead required secret in the broker compose file *(cleanup)*

`docker/compose.sandbox-broker.yaml:17` still hard-requires
`BUZZ_SANDBOX_TOKEN` with `${BUZZ_SANDBOX_TOKEN:?…}`, but **no broker code
reads that variable** — the Buzz-identity change removed it. The running
container still carries it. Harmless at runtime, but it forces the next
deployer to invent a secret that does nothing.

### F4. NIP-98 verification pins the broker's own public URL *(operational trap)*

`identity.rs` builds the signed URL from `BUZZ_SANDBOX_PUBLIC_URL`, ignoring the
Host header. The broker's is `http://127.0.0.1:9310`, so an SSH tunnel on any
other local port (e.g. `-L 19310:…`) fails signature verification and looks
exactly like a broken signing change.

**Tunnel on `9310` locally.** Worth a line in the docs; it will cost someone an
hour otherwise.

---

## Non-findings — my own test-payload mistakes, recorded so they aren't re-chased

- `launch.command: "buzz-acp"` makes the harness spawn *itself* as the agent
  (`buzz-acp acp` → `unexpected argument 'acp'`). The agent command is
  `buzz-agent`, `claude-agent-acp`, etc. — not the harness.
- Setting provider env in `agent.env_vars` while `launch` is present drops it.
  That is the spec rule (`env.rs:187-194`): when `launch` exists the desktop has
  already merged user env into `launch.env`, and a provider must not re-merge.
  Put it in `launch.env`.
- `buzz-agent` requires `BUZZ_AGENT_PROVIDER`; it is not a
  no-configuration default agent.
- `channels create` needs `--type stream|forum` and `--visibility open|private`.
- With `respond_to=owner-only`, a mention from anyone other than
  `launch.owner_pubkey` is ignored silently. The owner must be a key you hold,
  not an arbitrary hex string.

---

## What the automated tests must assert

Beyond the list already in `HANDOFF-TESTING.md`:

- **`e2e_sandbox_broker.rs`** — assert `CpusetCpus` is set, not just
  `NanoCpus`; the pinning is what makes `nproc` honest and it has no other
  guard.
- **`e2e_sandbox_agent.rs`** — assert the *live membership pickup* path
  (deploy → 0 channels → add member → `subscribing to new channel`), not only
  the case where the channel exists before launch. That transition is what a
  user actually does and it works today.
- **A provider-matrix test** — deploy with `BUZZ_AGENT_PROVIDER` set to a
  non-Anthropic provider and assert the env reaches the container. This is what
  proves the sandbox is not Claude-specific, and it is cheap (no LLM call
  needed — assert on the container env and a clean agent init).
- **An image-contract test** — assert each adapter named in the deploy contract
  actually exists on the image's PATH. F1 would have been caught at build time
  by a one-line check.

---

## Definition-of-done status

| Item | State |
|---|---|
| Manual pass complete, findings recorded | done (this file) |
| Provider→broker signing proven | done |
| Limits proven from Docker | done |
| Live membership pickup proven | done |
| Provider-agnostic env passthrough proven | done |
| **Agent posts a literal reply** | **blocked on a valid API key or F2** |
| Three E2E files written | not started |
| `just ci` green | not re-run since the manual pass |
| No sandboxes left on the host | verified clean |
