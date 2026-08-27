# Buzz Sandbox / Virtual Computer — Implementation Plan

**Status:** draft for review
**Author:** planning pass over `block/buzz` @ `69107dc3b`

---

## 1. Executive summary

The proposal — give Buzz agents a disposable Linux computer via a sandbox MCP
server and a provider abstraction — is architecturally sound and fits Buzz's
grain. But the codebase is **further along than the proposal assumes**, and two
of the proposal's premises are wrong in ways that change the build.

**What already exists (do not rebuild):**

| Proposed component | Reality |
|---|---|
| `SandboxProvider` trait + `providers/{docker,k8s,…}` | **Already exists** as the `buzz-backend-<id>` provider protocol, formally specified in `docs/remote-agents.md` (1,779 lines) with a working Kubernetes binding (`crates/buzz-backend-kubernetes`, 6,344 lines) |
| Sandbox Broker on a separate host | **Already exists** in substance: the desktop→provider→substrate split, with M1 "no management channel" as a design axiom |
| Disposable container per agent, non-root, resource-capped | **Already exists**: `Dockerfile.sprig` — Alpine, non-root `agent` user, `/workspace`, CPU/mem requests+limits (`pod.rs:88-123`), digest-pinned images |
| TTL / auto-destroy | **Already exists**: idle timeout + auto-stop in `buzz-acp` config; GC in `buzz-backend-kubernetes/src/gc.rs` |
| Relay must never touch Docker socket | **Already true**: the relay has no role in agent launch at all |

**The two wrong premises:**

1. **"`buzz-acp` accepts a single MCP binary, so `buzz-sandbox-mcp` must
   *replace* `buzz-dev-mcp`."** — Only half true. `buzz-acp` does collapse to
   one server (`lib.rs:5001-5011` builds a one-element `Vec`), but
   **`buzz-agent` already supports 16 MCP servers** with namespacing, restart,
   and backoff (`mcp.rs:26`, `mcp.rs:189-259`). The single-server limit is a
   ~10-line config change in one crate, not an architectural constraint. So
   **add alongside; do not replace.** Replacing `buzz-dev-mcp` would be a large,
   risky rewrite for no benefit.

2. **"Move signing authority out of the model-controlled process."** — This
   collides head-on with a *stated spec invariant*. `docs/remote-agents.md`
   invariant **I1 (identity fail-closed)** requires the deploy payload carry
   `private_key_nsec`, "never empty", and providers MUST construct
   `BUZZ_PRIVATE_KEY` from it. `buzz-dev-mcp` deliberately inherits
   `BUZZ_PRIVATE_KEY` into every shell child (`shell.rs:171`: *"BUZZ_PRIVATE_KEY
   is intentionally inherited — the buzz CLI needs it"*). Stripping it silently
   breaks agent messaging, git push, and git signing. This is a **spec
   amendment**, not an implementation detail — and it should be scoped as its
   own workstream (§6), not smuggled into the sandbox build.

**The genuinely new work is smaller than proposed and mostly lives in two
places:** a second MCP server, and one new provider binary.

---

## 2. What the sandbox actually adds

Buzz agents already run in disposable, resource-capped containers. So the honest
framing is *not* "agents get a computer" — they have one. The gaps are:

| Gap | Status today | Value |
|---|---|---|
| **Local/self-hosted substrate** | Only Kubernetes exists. No Docker provider. | High — unblocks solo devs and Emorise without a cluster |
| **Port exposure / live preview** | Nothing. `pod.rs` defines no ports. | High — "run the app and show me" is the killer demo |
| **Snapshot / restore** | Nothing | Medium |
| **Rich toolchain image** | `Dockerfile.sprig` is Alpine + bash/curl/git only — no Node, Python, Playwright, Postgres | High — agents cannot currently build most real projects |
| **Sandbox lifecycle as relay events** | No kinds exist | Medium — enables the "Computer" UI card |
| **Agent-visible sandbox controls** | Nothing | Medium |

**The single highest-value item is the rich image**, not the isolation
technology. An agent in a hardened Firecracker microVM that lacks `node` still
cannot build a web app.

---

## 3. Recommended architecture

Keep the existing layering. Add two components and extend a third.

```
Desktop / launcher
      │  deploy (JSON, stdin/stdout)   ← existing provider protocol, unchanged
      ▼
buzz-backend-docker      ← NEW provider (sibling of buzz-backend-kubernetes)
      │
      ▼
 sprig container ── buzz-acp ── buzz-agent
                                    │ MCP (multiple servers)
                        ┌───────────┴───────────┐
                        ▼                       ▼
                  buzz-dev-mcp           buzz-sandbox-mcp   ← NEW
                  (unchanged)            ports, snapshots,
                                         status, reset
```

### Why not the proposed "broker + sandbox-mcp replaces dev-mcp"

- The **broker already exists** as the provider protocol. Adding a second
  HTTP broker duplicates lifecycle, auth, and quota logic that
  `docs/remote-agents.md` already specifies and `buzz-backend-kubernetes`
  already implements.
- **Replacing `buzz-dev-mcp`** means reimplementing `shell`, `str_replace`,
  `read_file`, `rg`, `tree`, `todo`, `view_image` (~4,000 lines) plus the
  multicall shim. `buzz-agent` supports 16 servers — there is no reason to.

### The non-obvious constraint: the multicall shim

`buzz-dev-mcp` is a **multicall binary**. On startup it creates a 0700 tempdir
and symlinks `rg`, `tree`, `buzz`, `git-credential-nostr`, `git-sign-nostr` back
to itself, then prepends that dir to `PATH` (`shim.rs:28-49`). It also writes the
nostr key to a 0600 keyfile and *removes* `NOSTR_PRIVATE_KEY` from the
environment, exposing git auth/signing only through `GIT_CONFIG_*`
(`shim.rs:51-66`).

**Any sandbox that executes agent commands must reproduce this surface**, or
agents silently lose `buzz`, `rg`, `tree`, and git signing. `Dockerfile.sprig`
already does this correctly (symlinks all eight names to `sprig`) — which is
another reason to build *on* sprig rather than beside it.

---

## 4. Phased plan

### Phase 0 — Substrate question — **RESOLVED 2026-08-15**

> **Decision: single-tenant. Firecracker is out of scope.** Emorise will not run
> customers' agents on shared hardware. Build `buzz-backend-docker`; revisit only
> if multi-tenancy becomes a real requirement (cheap later — a new sibling
> binary, not a refactor). The rest of this section records the reasoning.

The proposal says "Docker for MVP, Firecracker for production." Before writing
code, settle **whether Firecracker is actually needed**, because it is the
single largest cost driver and it may be unnecessary:

- Buzz agents today run **owner-authorized code** on the owner's own cluster.
  The threat model in `docs/remote-agents.md` explicitly declines to defend
  against a hostile provider or hostile substrate.
- Firecracker's value is **multi-tenant isolation of untrusted workloads** —
  i.e. Emorise running *customers'* agents on *shared* hardware.
- If every deployment is single-tenant, **Firecracker buys little** over a
  hardened rootless-Docker provider, and costs bare-metal/KVM hosts, a rootfs
  build pipeline, jailer configuration, and a vsock control plane.

**Recommendation: build `buzz-backend-docker` now; defer Firecracker until a
concrete multi-tenant requirement exists.** The provider protocol makes this a
genuinely cheap deferral — Firecracker later is a new sibling binary, not a
refactor. This is the main product decision in this plan.

### Phase 1 — Rich toolchain image — **BUILT & VERIFIED 2026-08-15**

> **Shipped:** `Dockerfile.sprig-dev` (new), `.github/workflows/sprig-image.yml`
> (now a variant × arch matrix publishing both images), `.dockerignore` (fix).
>
> Verified by building and running the image locally, not by inspection:
> - Node 22.23.2, pnpm 10.34.5, Python 3.11.2, gh 2.97.0, ripgrep 13, git 2.39.5
> - All 7 multicall names resolve on PATH; `buzz --help` runs
> - Runs as `agent` **uid=10001**, matching `RUN_AS_UID` — the minimal image is
>   uid 1000 and relies on the pod securityContext to override it
> - System git signing config intact; `/workspace` writable by `agent`
> - Entrypoint still `exec`s the harness: SIGTERM reaches PID 1 and is handled
>   in 0s (L1 signal obligation)
> - `buzz-acp` fails closed with no identity (I1)
> - Real workflow proof: `npm init` → `node index.js` → `python3` → `git init`
>   all succeed in `/workspace`. The same commands fail on the minimal image,
>   which has no node/python/gh/cc at all.
> - Size: 1.08GB vs 59.4MB. Keep both; the minimal image stays the default for
>   chat-shaped agents.
>
> **Two incidental fixes found by building it:**
> 1. `.dockerignore` excluded `target/` but not `.hermit/` — ~1GB of host
>    toolchain cache was being copied into every build context, including the
>    existing sprig image's. This exhausted the builder's disk mid-export.
> 2. The base-image digests must be resolved with
>    `docker buildx imagetools inspect`, not assumed.
>
> Remaining before merge: the image is unsigned/unpublished until CI runs, and
> `provider_config.image` must be pointed at the `-dev` digest per agent.

Add `Dockerfile.sprig-dev` alongside the existing minimal image.

- Base on Debian slim rather than Alpine (glibc; Playwright and many
  prebuilt binaries do not ship musl builds).
- Include: Node LTS, Python 3, ripgrep, git, gh, curl, build-essential,
  and optionally Playwright + Chromium.
- Keep the **same eight multicall symlinks** and the same non-root `agent`
  user, `/workspace` layout, and system git-signing config — the entrypoint
  contract must not diverge.
- Keep both images: minimal for chat agents, dev for coding agents. Image is
  already a per-agent config field (`pod.rs:81`, `provider_config` v1).

**Ships value with zero protocol change.** This alone makes agents able to build
real projects.

### Phase 2 — Sandbox broker on the VPS + `buzz-backend-docker`

> **Decided 2026-08-15: option B — a broker service on Matterhorn.**
> Sandboxes run on the VPS, not on the desktop.

**Why a broker rather than a remote Docker socket.** The provider protocol
executes the provider binary on whatever machine runs Buzz Desktop
(`docs/remote-agents.md` §Discovery — the desktop scans its own PATH). A thin
provider calling Docker directly would therefore put containers on the laptop.
Pointing it at `DOCKER_HOST=ssh://matterhorn` works but pushes VPS SSH
credentials onto every client machine. The broker keeps substrate credentials
on the substrate and centralizes quotas, TTLs, and the allowed-image list.

```
Desktop ──deploy(JSON)──▶ buzz-backend-docker ──HTTPS+token──▶ broker (VPS)
                          (thin client, no                     │ docker API
                           Docker access)                      ▼
                                                        sandbox containers
```

The provider stays a conforming `buzz-backend-<id>` binary — same two
operations — so this is a transport swap, not a protocol change. If a
laptop-local mode is ever wanted, the same provider can target a local socket.

**Verified on Matterhorn 2026-08-15** (rather than assumed):

| Fact | Measured |
|---|---|
| Capacity | 8 cores, 62 GB RAM (49 GB available), 297 GB disk free, load ~2 |
| Existing load | 83 containers (Dokploy production sites) |
| Docker | 29.4.3, **rootful**, no userns-remap, Swarm active |
| `--memory` | enforced exactly (`memory.max` = 64 MB for `--memory=64m`) |
| `--pids-limit` | enforced (`pids.max` = 32) |
| `--cap-drop=ALL` | `CapEff: 0000000000000000` — all capabilities gone |
| `--cpuset-cpus` | correctly restricts visible cores |
| `--cpus` alone | **does NOT hide cores** — `nproc` still reports 8 |

Two consequences for the broker:

1. **Pin `--cpuset-cpus`, not just `--cpus`.** `--cpus` throttles CPU *time*
   but leaves `nproc` reporting all 8 cores, so `cargo build`, `make -j$(nproc)`,
   and Jest will over-parallelize and thrash. Set both.
2. **Docker is rootful and shares the host with production.** Container root is
   host root if anything escapes. Given single-tenant, owner-authorized code
   (§Phase 0) this is an accepted risk rather than a blocker, but the broker
   must compensate: `--cap-drop=ALL`, `no-new-privileges`, a non-root user,
   `--read-only` with explicit tmpfs, a dedicated network, no host mounts, and
   **never** the Docker socket. Consider `userns-remap` later; enabling it on a
   box running 83 production containers is its own migration.

**Broker shape** — small HTTP service, bearer-token auth, bound to loopback and
published through Traefik with an IP allowlist (the existing
`*-access-control.yml` pattern on this host):

```
POST   /sandboxes        create (image, cpu, mem, ttl, env) → id
GET    /sandboxes/:id    status
DELETE /sandboxes/:id    destroy
POST   /sandboxes/:id/ports   expose a port (Phase 4)
```

Broker-enforced policy the agent cannot override: max CPU/memory/disk, TTL and
idle reaping, the allowed-image list (digest-pinned), egress policy, and
per-owner concurrency caps.

#### Blocking prerequisite — relay reachability

`.env:53` sets `RELAY_URL=ws://localhost:3000`, and **Matterhorn runs no Buzz
relay** (83 containers, none Buzz). An agent in a VPS sandbox connects *back* to
the relay, so a laptop-local relay is unreachable from it. Before Phase 2 can be
demonstrated end to end, one of:

- deploy a relay to Matterhorn (`relay.staging.emorise.com`, Dokploy compose —
  the relay image already ships via `sprout-oss`), **recommended**; or
- point sandboxes at an existing reachable staging relay; or
- tunnel the laptop relay (dev-only, not a deployment posture).

This is infrastructure work, not Buzz code, and it gates the demo rather than
the build.

> **BUILT & PROVEN END TO END 2026-08-15.** `crates/buzz-sandbox-broker` (18
> tests) and `crates/buzz-backend-docker` (50 tests), both clippy-clean at
> `-D warnings`. Verified live, not by inspection: the provider deployed an
> agent through the broker to a sandbox on Matterhorn; the harness started with
> the correct identity, **connected to `wss://relay.staging.emorise.com`,
> published presence online**, and the sandbox had a working toolchain
> (node/python/git/gh/rg/cc) and could build and run code.
>
> Two findings from running it:
> * The agent needs `BUZZ_AGENT_PROVIDER` **and** the provider's model env var
>   (e.g. `ANTHROPIC_MODEL`) or `buzz-agent` exits at startup. Neither is
>   supplied by the deploy payload — they arrive via `launch.env`, so an agent
>   record without them deploys successfully and then dies. Worth surfacing in
>   the desktop as a pre-deploy check.
> * `buzz-backend-docker` reuses `env.rs` and `wire.rs` from the Kubernetes
>   binding almost verbatim. That is deliberate — the precedence and identity
>   rules are the spec's, not Kubernetes' — but it is now duplicated in two
>   crates. If a third binding appears, extract them into a shared crate rather
>   than copying a third time.

#### The provider binary itself

New crate `crates/buzz-backend-docker`, modelled directly on
`buzz-backend-kubernetes`.

Conformance obligations (L2, `docs/remote-agents.md:1447`):
- `info` + `deploy` ops only — **no exec/status/kill** (invariant M1). One
  process per operation: one JSON object in on stdin, one out on stdout.
- **Exit codes carry exactly one bit** — zero = output trustworthy, nonzero =
  failure regardless of stdout. Never encode meaning in nonzero values;
  failures are in-band `{"ok": false, "error": …}`.
- `info` must be **pure** — no substrate contact (it renders the config form
  before credentials are known to exist).
- Construct identity env from top-level payload fields, never `env_vars`
- Enforce the reserved-key strip
- **I2: no credential fields in `provider_config`** — no schema field may
  word-split into `secret|password|token|key|credential`, or every deploy fails
  desktop-side validation. Paths (`ssh_key_path`) are fine; secrets are not.
- Refuse the deploy if **both** `auth_tag` and `launch.owner_pubkey` are null,
  and refuse `relay-mesh` agents — both **before any mutation**
- Return a stable `agent_id` (container name)
- Honor at-most-one-live-instance (I4); "live" means the harness process is
  confirmed *running*, not merely that the body was accepted
- Redact secrets from all output

Hardening (matching the proposal's list, which is sound):
`--cpus`, `--memory`, `--pids-limit`, `--read-only` + tmpfs where possible,
non-root user, dedicated bridge network, **no host mounts**, **no Docker socket
mount**, `--cap-drop=ALL`, `--security-opt=no-new-privileges`.

Prefer **rootless Docker** or **Podman** as the documented deployment posture.

Docker-specific analogs of the Kubernetes pod shape:
- **Never** pass `--entrypoint` — mirrors `pod.rs:111-113`; the image entrypoint
  must exec the harness as PID 1 so it receives SIGTERM directly (an L1
  obligation: "a wrapper that swallows the signal conforms to nothing")
- `--stop-timeout 60` — analog of `TERMINATION_GRACE_SECONDS`
- A container **label as the management marker** (analog of
  `app.kubernetes.io/managed-by`) plus a full-pubkey label as identity evidence;
  every destructive delete must be fenced to the exact observation that
  authorized it
- Restart policy `no` — an L1 obligation is that a supervisor **never restarts
  an intentional clean exit**

Reusable nearly as-is from the Kubernetes crate: `wire.rs`, `env.rs`,
`intent.rs`, `naming.rs`. Prior art exists for a non-Kubernetes substrate — the
spec names a **systemd/SSH deployer (PR #3449)** as a live example conforming to
L1+L2 with its own L3.

> **Known Defect 3 is already fixed — the spec's defect list is stale.** The
> spec warns that until `deploy_payload_json` emits the `launch` block, "no
> provider can conform to §Launch data". Verified: `build_launch_block` exists
> at `desktop/src-tauri/src/commands/agents_deploy.rs:49`, is emitted at `:215`,
> and is covered by tests (`:249`). **Phase 2 is unblocked.**
>
> More broadly: `docs/remote-agents.md` marks the Kubernetes binding, the sprig
> image, and `backend.rs` as *"to be added"*, but all three exist and are
> substantial. **Treat the doc's status annotations as unreliable and verify
> against code** — several "defects" may likewise be closed.

### Phase 3 — `buzz-acp` multi-MCP support

Change `mcp_command: String` → an ordered list, preserving the current
single-value form for compatibility.

- `crates/buzz-acp/src/config.rs:262,509` — field type + CLI/env parsing
  (`BUZZ_ACP_MCP_COMMAND` stays valid; add `BUZZ_ACP_MCP_COMMANDS`)
- `crates/buzz-acp/src/lib.rs:5001` — `build_mcp_servers` returns N servers
  instead of a one-element vec; existing name-derivation logic per entry
- Cap at `buzz-agent`'s `MAX_MCP_SERVERS = 16` (`mcp.rs:26`); note
  `MAX_TOOLS_PER_SESSION = 128` (`mcp.rs:21`) also applies
- `desktop/src-tauri/src/managed_agents/discovery.rs:153,187` — the runtime
  table hardcodes `mcp_command: Some("buzz-dev-mcp")`; this becomes a list.
  Beware `migration.rs:1220` (`reconcile_provider_mcp_commands`) **overwrites
  the persisted `mcp_command` from this table on every launch** — a per-agent
  override will be silently reverted unless that reconciler is updated too.
- `RESERVED_ENV_KEYS` (`reserved_env_keys.rs:43`) contains
  `BUZZ_ACP_MCP_COMMAND`, so user env cannot override it — the new key needs
  the same treatment.

Small, well-tested surface; existing tests at `lib.rs:6875-6910` pin the
current behavior and should be extended rather than replaced.

**Bonus fix in the same change.** The persona `.mcp.json` path
(`buzz-persona/src/manifest.rs:112` → `pack.rs:201` → `resolve.rs:277`
`merge_mcp_servers`) is fully parsed, validated, and merged into
`ResolvedPersona.mcp_servers` — and then **discarded**. The only consumer
outside the crate is a `println!` of the count in `buzz-cli/src/commands/pack.rs:115`.
`buzz-acp` depends on `buzz-persona` in `Cargo.toml:22` but never imports it.
Once `build_mcp_servers` takes a list, wiring personas through is a short step
and makes "this persona gets a sandbox" declarative.

### Phase 4 — `buzz-sandbox-mcp` (the genuinely new tools)

A **second** MCP server, additive to `buzz-dev-mcp`. It does *not* wrap
`shell`/`read_file`/`write_file` — those already run in the sandbox, because the
agent itself is in the sandbox.

Tools:
- `sandbox_status` — image, CPU/mem limits, uptime, remaining TTL
- `sandbox_expose_port` — publish a port, return a reachable URL
- `sandbox_snapshot` / `sandbox_restore` — provider-dependent; may be
  unsupported, and must degrade honestly
- `sandbox_reset` — restore `/workspace` to its initial state

`expose_port` is the one with real security weight: it turns an agent into a
network listener. It needs an explicit allow-policy, a bounded port range, and
owner-visible surfacing — not a bare "open a port" primitive.

### Phase 5 — Sandbox lifecycle events + UI

Add kinds in `crates/buzz-core/src/kind.rs`. The `48xxx` audit range is the
natural home (`KIND_AUDIT_ENTRY = 48001`); the proposal's `SANDBOX_*` names map
cleanly.

Suggested: `KIND_SANDBOX_CREATED`, `KIND_SANDBOX_PORT_EXPOSED`,
`KIND_SANDBOX_SNAPSHOT_CREATED`, `KIND_SANDBOX_DESTROYED`.

This is what powers the proposed "Computer" card (Ubuntu / 4 CPU / 8 GB / 17
min / Open Terminal / Open Preview). Note the card's **Open Terminal** button is
in direct tension with invariant M1 (no management channel) — see §6.

---

## 5. What I recommend cutting from the proposal

| Proposed | Verdict |
|---|---|
| Separate `buzz-sandbox-broker` HTTP service | **Cut.** Duplicates the provider protocol. |
| `buzz-sandbox-mcp` replaces `buzz-dev-mcp` | **Cut.** Add alongside; `buzz-agent` already supports 16 servers. |
| New `SandboxProvider` trait + `providers/` tree | **Cut.** The `buzz-backend-<id>` protocol is that abstraction. |
| Firecracker in v1 | **Defer.** See Phase 0. |
| Auto-provision per session | **Already true** — deploy already creates a fresh container per agent start. |

---

## 6. Open questions (need a decision)

1. **Multi-tenancy** — will Emorise run *customers'* agents on shared hardware?
   Yes ⇒ Firecracker matters and Phase 0 flips. No ⇒ Docker is sufficient
   indefinitely. **This is the load-bearing product question.**

2. **The private key.** Agents need `BUZZ_PRIVATE_KEY` for the `buzz` CLI, git
   push, and git signing — intentional today (`shell.rs:171`) and required by
   invariant I1, which mandates `private_key_nsec` in the deploy payload and
   says reading identity from `env_vars` instead "yields an identityless agent."

   Note the existing partial mitigation, which is the right pattern to extend:
   `buzz-dev-mcp` already **removes `NOSTR_PRIVATE_KEY` from the environment**,
   writes it to a 0600 keyfile, and exposes git auth/signing only via
   `GIT_CONFIG_*` (`shim.rs:51-66`). Doing the same for `BUZZ_PRIVATE_KEY` means
   giving the `buzz` CLI a keyfile or a local signing socket instead of an env
   var — the model-controlled shell then holds no key material directly.

   **Real, worth doing, but its own workstream** — it touches the spec, the CLI,
   git signing, and every launcher. Do not couple it to the sandbox build.

3. **"Open Terminal" vs invariant M1.** The spec's axiom is that the desktop
   holds *no* management channel to a running agent — status is relay presence,
   stop is a relay message. An interactive terminal is exactly such a channel.
   Either the terminal is brokered *through the relay as events* (consistent
   with M1, more work), or M1 is amended. Worth resolving before the UI is
   designed.

4. **Egress policy.** Should sandboxes reach the public internet by default?
   Agents need `npm install`, but unrestricted egress in a multi-tenant world is
   a data-exfiltration path. Suggest: allowlist-by-default with a per-agent
   opt-out.

---

## 7. Suggested order

1. Phase 1 (rich image) — immediate value, no protocol risk
2. Phase 3 (multi-MCP) — small, unblocks Phase 4
3. Phase 2 (Docker provider) — unblocks non-Kubernetes users
4. Phase 5 (events) + Phase 4 (sandbox MCP) — the visible "computer" product
5. Firecracker — only if §6.1 says multi-tenant

Phases 1–3 are independently shippable and each stands on its own.
