# Handoff — Buzz agent sandboxes

**Written 2026-08-16.** Everything below is verified running on Matterhorn
unless explicitly marked otherwise. Nothing is committed — it all sits in the
working tree.

Read `SANDBOX-PLAN.md` first for the design and why each decision was made.
This file is the operational state.

---

## What this is

Buzz agents can now run on a VPS instead of the user's Mac, in a disposable
sandbox with a real Linux desktop, a browser the user can take over, and Claude
Code as the reasoning engine. No LLM API key anywhere.

The user is **Ron** — a technical product-builder who does not read code. Report
in outcomes ("an agent replied in #general"), not implementation. He verifies by
clicking the running app, so "tests pass" is not done. Always end with one
concrete next step.

---

## Running right now on Matterhorn

SSH is port **2222** (`ssh -p 2222 root@matterhorn.emorise.com`).

| Service | Where | State |
|---|---|---|
| Relay | `/opt/buzz-relay-staging` (docker compose) | healthy, `wss://relay.staging.emorise.com` |
| Broker | `/opt/buzz-sandbox-broker` (plain `docker run`) | healthy, `127.0.0.1:9310` |
| Postgres / Redis / MinIO | same compose stack | healthy |

Secrets live in `.env` in each directory (0600), generated on the server. The
broker's Buzz identity is pinned via `BUZZ_SANDBOX_RELAY_KEY` in its `.env`.

Images on the host, each layered on the previous:

```
buzz-sprig-dev      toolchain: node, python, git, gh, rg, cc
buzz-sprig-desktop  + Xvfb, openbox, chromium, x11vnc, noVNC
buzz-sprig-claude   + Claude Code CLI and the ACP adapter
```

---

## Verified working (each tested live, not assumed)

- **Sandboxes** — created through the provider → broker → container path, with
  CPU, memory, PID, and capability limits enforced *inside* the container.
- **TTL reaping** — a sandbox past its lifetime is destroyed automatically and
  announced. Confirmed with a 120s TTL.
- **Agent desktop** — Chromium on a virtual screen; the noVNC viewer loads from
  a laptop with a full input client. The agent drives the browser over CDP while
  a human can take the mouse — proven by navigating to GitHub's login page.
- **Browser tools** — six MCP tools (`browser_navigate`, `_read`, `_click`,
  `_type`, `_screenshot`, `_tabs`) driven through the real MCP protocol against
  a live site.
- **Claude as the brain** — credentials copied from the Mac Keychain into the
  sandbox; `claude auth status` reports logged in and a real inference call
  returned. The full harness runs with `claude-agent-acp`.
- **Buzz identity end to end** — the provider signs each broker request with the
  agent's own key (NIP-98); the broker verifies it and defers authorization to
  the relay's membership answer. No shared secret anywhere.
- **Sandbox events** — kinds 48200 (created) / 48201 (destroyed) publish to the
  relay and are accepted.

Tests: 29 (broker) + 51 (provider) + 101 (dev-mcp) + 249 (core), all passing,
clippy clean at `-D warnings`, zero `unwrap`/`expect` in production paths.

---

## Next task

**Get an agent replying in a real Buzz channel.** Everything underneath works;
the agent connects and goes online but belongs to no channel, so it has nothing
to answer. That is the last gap between "the machinery works" and "an agent does
useful work".

Likely shape: create a channel on the staging relay, add the agent's pubkey as a
member, send a message mentioning it, and confirm a reply. `buzz-cli` has the
subcommands (`channels`, `messages`); `crates/buzz-cli/TESTING.md` is the
runbook.

---

## Gotchas that cost real time — do not rediscover these

1. **Adding an event kind takes two edits.** `buzz-core/src/kind.rs` is not
   enough: `crates/buzz-relay/src/handlers/ingest.rs` maps each kind to a write
   `Scope`, and anything unmapped is rejected as
   `"restricted: unknown event kind"`. A full relay rebuild without that mapping
   changes nothing.
2. **Relay migrations are opt-in** — set `BUZZ_AUTO_MIGRATE=true`. `CLAUDE.md`
   and the README both claim they auto-apply. They do not.
3. **The relay refuses to boot without S3.** It runs a git object-store
   conformance probe at startup, so MinIO is mandatory.
4. **That probe logs hundreds of HTTP 412 warnings on a healthy boot** — it is
   deliberately testing a race. Grep `"level":"ERROR"`, never the substring
   "error".
5. **Named volumes mount root-owned** when the mount path does not exist in the
   image. The relay runs as uid 1000 and crash-loops without an init container
   that chowns it.
6. **Traefik allowlists: never use `10.0.0.0/8`** as a catch-all for Docker
   networks — it swallows the `10.13.13.0/24` WireGuard VPN and silently
   disables the allowlist. Verify enforcement by narrowing until a known client
   gets 403.
7. **Containers on a non-Traefik Docker network** reach Traefik as
   **`172.18.0.1`** (the `docker_gwbridge` gateway), not their own subnet.
   Allowlist that address. Diagnose with a `traefik/whoami` container rather
   than guessing.
8. **`--cpus` does not hide host cores.** `nproc` still reports all 8, so build
   tools over-parallelize. Pin `--cpuset-cpus` as well.
9. **Chromium needs `--no-sandbox`** in these containers: capabilities are
   already dropped, so its own sandbox cannot initialise. The container is the
   sandbox.
10. **`chromium --headless --screenshot` fails** without an explicit
    `--user-data-dir`. And `xwd` is not in Debian's `x11-utils`.
11. **macOS tar ships `._*` AppleDouble files** that cargo mistakes for source
    (`can't find bin ._mention`). `find . -name '._*' -delete` after extracting
    on the server.
12. **The `buzz-sandbox-network` vanished once** between runs. If a sandbox
    fails to start with "network not found", recreate it:
    `docker network create --driver bridge --subnet 172.16.240.0/24 buzz-sandboxes`.
    The 172.16–31 space on this host is nearly exhausted; take a `/24`.
13. **Env-mutating Rust tests must be serialized** (a `Mutex`), or they corrupt
    each other in parallel and pass only under `--test-threads=1`.

---

## Decisions already made — do not relitigate

- **Single-tenant, so no Firecracker.** Docker is sufficient; the provider
  protocol makes Firecracker a cheap later addition if that changes.
- **No LLM API key.** Claude Code subscription only. Databricks was investigated
  and dropped (Azure workspace, needs the `adb-…` host, model serving unknown).
- **No relay inference proxy.** One was built for the API-key path and removed
  during cleanup, since that path was ruled out. If central key management is
  ever wanted, it is a fresh endpoint on the relay, not a revert.
- **Broker stays a separate process** from the relay, because it can create
  containers and the relay handles untrusted internet traffic.

---

## Known gaps

- **No disk quota per sandbox.** Needs XFS with project quotas; Matterhorn is
  ext4, so `BUZZ_SANDBOX_DISK` stays unset. CPU, memory, and PIDs are capped;
  disk is not. A runaway build could fill the disk production sites share.
- **Nothing consumes the sandbox events yet.** The desktop does not render a
  "this agent has a computer" card — the events exist and are accepted, but no
  UI reads them.
- **The viewer needs a tunnel.** `BUZZ_SANDBOX_VIEWER_URL` is unset, so events
  carry no viewer link. Opening a sandbox desktop currently means an SSH tunnel
  to its port 6080.
- ~~`env.rs` and `wire.rs` duplicated between the two providers~~ — **fixed**.
  Both now live in `buzz-backend-common`; substrate-specific error wording is
  passed in via `SubstrateDiagnostics` rather than forking the rule.
- **Credentials in the sandbox — now avoidable.** The agent's brain can run on
  the trusted side and drive the sandbox's tools over authenticated HTTP
  (`BUZZ_DEV_MCP_BIND` on the sandbox, `--mcp-url` on the harness), so no LLM
  credential need enter the sandbox at all. See `SANDBOX-ARCHITECTURE-OPTIONS.md`.
  The in-sandbox arrangement still works and remains the default.

---

## Repo changes (uncommitted)

New: `crates/buzz-sandbox-broker/`, `crates/buzz-backend-docker/`,
`crates/buzz-dev-mcp/src/browser.rs`,
`Dockerfile.sprig-{dev,desktop,claude}`, `Dockerfile.sandbox-broker`,
`docker/compose.{relay-staging,sandbox-broker}.yaml`,
`docker/traefik/buzz-relay-staging.yml`, `SANDBOX-PLAN.md`.

Modified: `kind.rs` (sandbox kinds), `ingest.rs` (scope mapping), `buzz-dev-mcp`
(browser tools), `.dockerignore` (excludes `.hermit`), `sprig-image.yml`
(variant matrix), root `Cargo.toml` (two new members).
