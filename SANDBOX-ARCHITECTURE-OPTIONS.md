# Where should the brain live? — the sandbox architecture decision

**Written 2026-08-16**, after the manual pass (`MANUAL-PASS-FINDINGS.md`).

Ron's question, which is the right one:

> Why do we need a token or access to an LLM from within the sandbox anyway?
> Is it not all controlled via MCP or tool use from the agent? … We want the
> Buzz agent to control the VM, the VM does not have to be intelligent but
> simply provide data back to the agent.

Short answer: **that architecture is correct, the protocol already supports it,
and what is built today does the opposite.** This document records what was
found and what it costs to change.

---

## What is built today (brain inside the VM)

```
┌─ sandbox (Matterhorn) ──────────────────────────────┐
│  buzz-acp        watches the relay, decides to act  │
│  agent CLI       ★ THE BRAIN — calls the LLM        │──► api.anthropic.com
│  buzz-dev-mcp    the hands (stdio, same container)  │    openrouter.ai
└─────────────────────────────────────────────────────┘
```

The agent CLI is the process that makes the outbound model call, so the API key
(or a vendor login) has to be **inside** the sandbox. That is why the manual
pass ended at `401: static key rejected` — nothing upstream was broken, the
box simply had no valid credential of its own.

MCP does not change this. MCP is the agent reaching *out* to tools; it supplies
hands, not a brain. `buzz-dev-mcp` is spawned as a **child process over stdio**
(`rmcp` features: `server`, `transport-io`, `macros` — no HTTP server
transport), so today the tools must be co-located with the thing calling them.

### What this costs

- **A live API key in every sandbox**, on a shared host, in a container running
  agent-authored code. One leak bills Ron directly.
- **No central spend control** — no per-agent limit, no audit of who spent what,
  no revocation short of destroying containers.
- **Credentials cannot be provisioned** for subscription agents at all
  (finding F2): a fresh sandbox reports `{"loggedIn": false}` and there is no
  injection path. The earlier "verified" note came from a hand-copied file.

---

## What Ron is describing (brain outside, VM is hands only)

```
┌─ Buzz agent (trusted side) ─────────┐
│  buzz-acp + agent CLI ★ THE BRAIN   │──► LLM (key never leaves here)
└──────────────┬──────────────────────┘
               │ MCP over HTTP (authenticated)
┌──────────────▼─ sandbox (dumb) ─────┐
│  buzz-dev-mcp --http                │
│  shell · files · browser · desktop  │
└─────────────────────────────────────┘
```

The VM holds no key, no vendor login, and no intelligence. It executes shell
commands, reads files, and drives a browser, returning data. Exactly "control
the VM, the VM provides data back".

### This is spec-blessed, not a fork

The Agent Client Protocol already defines remote MCP. `McpServer` is an enum
with an `Http` variant (`type: "http"`, plus `url`, `headers`, `name`) and an
SSE variant, gated on the agent advertising `mcp_capabilities.http`.

**The live run already proved the adapter supports it.** From the sandbox log
during the manual pass:

```
claude-agent-acp   mcpCapabilities: {"http":true,"sse":true}   ← supports remote MCP
buzz-agent         mcpCapabilities: {"http":false,"sse":false} ← stdio only
```

So Claude Code can already be pointed at a remote MCP server. Buzz's own
minimal agent cannot yet.

### What blocks it in Buzz today

Two concrete gaps, both narrow:

1. **`buzz-dev-mcp` has no HTTP transport.** It is stdio-only. `rmcp` ships
   `transport-streamable-http-server` as a cargo feature; serving over HTTP is
   an additive change to `main.rs` plus a feature flag, not a rewrite of the
   tools. All fourteen tools (shell, read_file, str_replace, view_image, six
   browser tools, todo, hooks) are transport-agnostic already.
2. **`buzz-acp` only builds the stdio `McpServer` variant.**
   `crates/buzz-acp/src/acp.rs:30` is `{name, command, args, env}` — the
   `McpServerStdio` shape, as its own doc comment says. It needs the `Http`
   variant to pass a URL instead of a command.

Neither touches the broker, the provider, the event kinds, or the relay.

---

## The security question this actually settles

Moving the brain out is not only tidier — it inverts the trust model.

| | Brain inside (today) | Brain outside (proposed) |
|---|---|---|
| API key location | every sandbox | one trusted process |
| Blast radius of a compromised sandbox | the key | a session token you revoke |
| Spend control | none | per-agent, central |
| Who can be attacked | the credential | the tool surface |

The remaining risk moves to the MCP endpoint: a sandbox exposing `shell` over
HTTP is remote code execution by design, so that endpoint must be
authenticated and reachable only by its owning agent. Buzz already has the
right primitive — the sandbox has a Buzz identity and the broker already
authenticates callers by NIP-98. The same scheme applies to the MCP port.

---

## The honest catch: subscription agents

This is the one place the clean design does not fully reach.

- **Pay-per-token** (Anthropic key, OpenAI, OpenRouter, Databricks): the brain
  runs wherever you like, because the key is just configuration. Full win.
- **Subscription** (Claude Code, Codex): those CLIs authenticate to their
  vendor with their own login and expect to *be* the agent process. If the
  brain is outside the VM, they run outside the VM — which is fine, and in fact
  better: the vendor login stays on the trusted machine and never lands on a
  shared host. That deletes finding F2 rather than solving it.

So the split is not "brokered vs in-sandbox keys". It is: **the brain always
runs on the trusted side; the sandbox is always dumb.** Subscription and
pay-per-token differ only in what the trusted side authenticates with.

---

## A rejected alternative, for the record

An earlier design (deleted during cleanup, per `HANDOFF.md`) had the **relay
proxy inference**: sandboxes call the relay, the relay attaches the real key.
The relay's route table today has no inference endpoint, confirming it is gone.

It is worse than Ron's proposal for this purpose:

- It keeps the brain in the sandbox and only hides the key, so a compromised
  sandbox still drives unlimited inference — it just cannot exfiltrate the
  credential.
- It puts the relay, which handles untrusted internet traffic, on the critical
  path of every token.
- It does nothing for subscription agents, which cannot be pointed at a proxy.

Ron's version removes the brain instead of hiding the key. Strictly better.

---

## Recommendation

**Adopt the brain-outside model.** It is what the protocol expects, it deletes
two of the four findings from the manual pass (F1's urgency and F2 entirely),
and it removes live credentials from a shared production host.

Sequencing matters, because the current branch is not wasted:

1. **Ship the current branch as-is**, with the sandbox documented as
   "agent-hosted" and the API key noted as a known limitation. Everything
   proven in the manual pass — signing, limits, TTL reaping, lifecycle events,
   live membership pickup, the desktop and browser — is independent of where
   the brain lives, and is needed either way.
2. **Then add remote MCP** as a follow-up: HTTP transport in `buzz-dev-mcp`,
   the `Http` variant in `buzz-acp`, NIP-98 auth on the MCP port. This is
   additive; no revert.

The alternative — hold the branch and build remote MCP first — delays
everything already verified for a change that does not invalidate any of it.

### What would need building (follow-up scope)

| Piece | Where | Rough size |
|---|---|---|
| HTTP server transport | `buzz-dev-mcp` (rmcp feature + `main.rs`) | small |
| `McpServerHttp` variant | `crates/buzz-acp/src/acp.rs:30` | small |
| NIP-98 auth on the MCP port | new, mirrors `buzz-sandbox-broker/src/identity.rs` | medium |
| Expose the MCP port | broker port mapping, already does this for 6080 | small |
| E2E: remote tools drive a sandbox | `crates/buzz-test-client/tests/` | medium |

The auth layer is the only genuinely new work, and there is a working model to
copy in the broker.

---

## Sources

- ACP schema, `McpServer` transport variants —
  <https://agentclientprotocol.com/protocol/schema>
- `rmcp` cargo features, `transport-streamable-http-server` —
  <https://docs.rs/crate/rmcp/1.1.0/features>
- `mcpCapabilities` values: observed live in the manual pass, not inferred.
