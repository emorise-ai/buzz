# Handoff — let an agent own its own computer

> **STATUS 2026-08-17: implemented.** Broker `POST /sandboxes/{id}/extend`
> (live expiry in broker state, owner-only, clamped, 48200 republish),
> `buzz sandbox create/list/status/extend/destroy`, `BUZZ_SANDBOX_BROKER_URL`
> injection via the docker provider (`agent_broker_url` config), and app-side
> "Start a computer" / "Stop computer" on the agent profile
> (`create_agent_sandbox` / `destroy_agent_sandbox` Tauri commands through the
> `/sandbox-viewer` proxy). The broker now accepts NIP-98 signatures over any
> of several configured origins (`BUZZ_SANDBOX_PUBLIC_URL` is comma-separated).
> Verified live against the staging broker. Remaining follow-up: §4
> activity-based keepalive.

**Written 2026-08-16.** Answers a direct product question: *why can't an agent
(e.g. Fizz) create its own sandbox, keep it alive while it works, and let it go
when done?*

**Today it can't.** A sandbox is provisioned *for* an agent when the agent is
launched onto remote compute, its lifetime is fixed at birth, and nothing can
extend it. This file is the brief for making the sandbox an agent-managed
resource: **create / status / extend / destroy**, driven by the agent itself.

Read alongside `HANDOFF-SANDBOX-UI.md` (the desktop viewer, now built) and the
memory note `buzz-sandbox-viewer-routing`.

---

## What exists today (and what doesn't)

**The broker** (`crates/buzz-sandbox-broker`) exposes:

| route | who calls it | purpose |
|---|---|---|
| `POST /sandboxes` | the provider | create a sandbox, owner = the `p` it's told |
| `GET /sandboxes` | any member | list managed sandboxes |
| `GET /sandboxes/{id}` | any member | one sandbox's state |
| `DELETE /sandboxes/{id}` (also `POST …/stop`) | any member | destroy now |
| `GET /sandboxes/{id}/desktop…` | signed token | the live screen (built) |

Every mutating call is authenticated by **NIP-98** (the caller signs with their
own Nostr key) and authorized by asking the relay whether that key is a member
(`identity.rs`). The verified pubkey becomes the sandbox's owner.

**Who creates one today:** only `buzz-backend-docker` (the provider), as part of
launching an agent onto the sandbox host. There is **no agent-facing surface** —
no `buzz sandbox` CLI, nothing an agent can invoke mid-task to say "I need a
browser." During testing, sandboxes were created by a hand-rolled script
(`desktop/_sandbox_create.mjs`) impersonating the provider. That script is a
crutch, not the design.

**Lifetime:** `POST /sandboxes` takes `ttl_seconds` (default 1h, max 8h,
`sandbox.rs` `DEFAULT_TTL_SECONDS` / `MAX_TTL_SECONDS`). The broker stamps an
expiry into a **Docker container label** `com.buzz.sandbox.expires-at`
(`LABEL_EXPIRES`). A 30-second `reaper` loop (`main.rs`) lists managed
containers, reads that label, and `remove_container`s any whose time has passed,
publishing a kind:48201 `expired`. **There is no extend, renew, or
keepalive.** A sandbox an agent is actively using is reaped out from under it the
moment the clock runs out — this is exactly why the demo sandbox kept vanishing.

---

## The job

Make the sandbox a resource the **agent** manages through `buzz-cli`, the way it
manages everything else. Per `CLAUDE.md` (§ Agent-facing operations): add a
subcommand to `buzz-cli`, wire the HTTP call in `client.rs`. The broker gains
the endpoints those commands need.

### 1. Broker: an extend endpoint (the hard part first)

Add `POST /sandboxes/{id}/extend` taking `{ "ttl_seconds": N }` (or
`{"extend_by_seconds": N}` — pick one and be consistent). It must:

- Authenticate + authorize exactly like the other mutating routes (`authorize`).
- **Only the owner may extend.** The create path records the owner as the `p`
  tag on the 48200 and (recommended) should also stamp an owner label on the
  container so the broker can check ownership locally without a relay round trip.
  Today ownership lives only in the published event; decide whether extend reads
  it back from the relay or from a new `com.buzz.sandbox.owner` label. **A label
  is simpler and offline** — add it in `container_spec`.
- Compute the new expiry as `now + ttl_seconds`, **clamped so total lifetime
  never exceeds `MAX_TTL_SECONDS` from creation** (don't let repeated extends
  make a sandbox immortal — that defeats the disk-safety purpose the TTL serves;
  see `sandbox.rs` note that disk is not quota-capped).
- Persist the new expiry, and **republish a kind:48200** with the new
  `expires_at` so the desktop's countdown updates (the UI already reconstructs
  from the latest 48200 — a re-announcement with a later `created_at` wins, see
  `desktop/src/features/agents/sandbox/sandboxState.ts`).

**The blocking design problem — Docker labels are immutable.** You cannot change
`com.buzz.sandbox.expires-at` on a running container. Three ways out, in
increasing order of work:

1. **Move expiry off the label into broker state.** Keep an in-memory
   `Map<container_id, expires_at>` (seeded from labels on startup for
   crash-recovery). The reaper reads the map, not the label; extend mutates the
   map. Simplest, but expiry is lost if the broker restarts unless you also
   persist it (a small sidecar file or a relay event to read back).
2. **A sidecar store** (a JSON file under a broker volume, or a tiny sqlite).
   Survives broker restart; more moving parts.
3. **Recreate the container** with a new label. Wrong for a desktop — it would
   kill the running Chromium session the human may be logged into. Do not.

**Recommendation: option 1, with the expiry seeded from the label at startup and
re-derived from the latest 48200 on the relay if the broker restarts.** The
label stays as the *initial* expiry and crash-recovery seed; the live authority
is broker state. Document this clearly — the label-vs-state split is the kind of
thing that silently rots.

### 2. Broker: create already works — expose it to the agent

`POST /sandboxes` is complete. The only gap is that no agent-facing client calls
it. The create contract an agent needs (from `sandbox.rs` `CreateRequest`):
`image` (required, must be in `BUZZ_SANDBOX_IMAGES`), optional `owner`, `cpus`,
`memory_mb`, `ttl_seconds`, `env`. For a self-service "give me a browser" the
agent will typically want the desktop image in **tools-only mode** (set `env`
`BUZZ_DEV_MCP_BIND` + `BUZZ_DEV_MCP_OWNER`, `BUZZ_DESKTOP_ENABLED=1`) so the
container stays alive without a full agent-in-a-box — see
`scripts/sprig-entrypoint.sh` and the working env in `_sandbox_create.mjs`.

Decide the product question: **should an agent pick its own image and budget, or
should the broker impose a self-service default?** Leaning toward a fixed
self-service profile (one desktop image, modest budget, short default TTL) so an
agent can't provision an 8-core box by asking — the broker already clamps, but a
policy here is clearer than trusting the prompt.

### 3. buzz-cli: the `sandbox` subcommand

Add a top-level `Sandbox` subcommand group (mirror the existing
`#[command(subcommand)]` groups in `crates/buzz-cli/src/lib.rs`), with:

- `buzz sandbox create [--image …] [--ttl …]` → `POST /sandboxes`, prints the id.
- `buzz sandbox list` / `buzz sandbox status <id>` → `GET`.
- `buzz sandbox extend <id> --ttl <seconds>` → the new `POST …/extend`.
- `buzz sandbox destroy <id>` → `DELETE`.

Wire each to the broker in `client.rs`, signing with **NIP-98** the same way the
provider does (`crates/buzz-backend-docker/src/broker.rs` `sign_request` is the
reference — `u`/`method`/`payload` tags, signed over the exact broker URL).

**Where does the CLI learn the broker URL?** The agent's env. The provider
already knows it (`broker_url`, default `http://127.0.0.1:9310` over the
authenticated tunnel). Inject it into the managed-agent env as
`BUZZ_SANDBOX_BROKER_URL` (mirror how `BUZZ_RELAY_URL` / `BUZZ_PRIVATE_KEY` are
injected — see `CLAUDE.md` § Agent CLI) so `buzz sandbox …` finds it with no
flag. Without this the agent can't reach the broker at all.

### 4. Keepalive (optional but the point of the request)

"Maintain its validity" implies the sandbox shouldn't die *while in use*. Two
levels:

- **Explicit:** the agent calls `buzz sandbox extend` when it knows it needs more
  time. Cheap, predictable, no magic. Ship this first.
- **Activity-based:** the broker bumps expiry when the desktop or tools port sees
  traffic (a request through the desktop proxy, or an MCP tool call on
  `TOOLS_PORT`). Nice, but it's a policy with failure modes (a forgotten tab
  keeps a box alive forever — cap it at `MAX_TTL_SECONDS` regardless). Treat as a
  follow-up, not v1.

---

## Sequencing

1. Broker `POST /sandboxes/{id}/extend` + move expiry authority into broker state
   (option 1) + owner label + republish 48200. Unit-test the clamp and the
   owner-only rule.
2. `buzz sandbox` CLI (create/list/status/extend/destroy) + `client.rs` wiring +
   `BUZZ_SANDBOX_BROKER_URL` injection into the managed-agent env.
3. Verify live: from inside a sandbox (or with the injected env), run
   `buzz sandbox create`, watch the 48200 land and the desktop appear in the app;
   `buzz sandbox extend` and watch the countdown jump; `buzz sandbox destroy` and
   watch the card disappear (48201).
4. (Follow-up) activity-based keepalive.

---

## Conventions that bite (from doing the UI work)

- **NIP-98 signs the exact URL.** The broker verifies against
  `BUZZ_SANDBOX_PUBLIC_URL` (`http://127.0.0.1:9310` on staging). Sign over that
  exact origin/path or every request 401s looking like a broken signature. Tunnel
  on port **9310** locally (`ssh -L 9310:127.0.0.1:9310`).
- **Adding a kind isn't enough for the relay to accept it** — but 48200/48201 are
  already mapped in `crates/buzz-relay/src/handlers/ingest.rs`. Extend reuses
  48200, so no relay change is needed.
- **The reaper runs every 30s.** An extend that races the reaper by <30s is fine;
  just make extend update the same authority the reaper reads (that's why the
  label-vs-state split matters).
- **Staging relay quota:** `BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC` was raised to
  200 on staging (default 10 throttled the desktop app's reload storm). Not
  relevant to the broker, but if you test through the desktop app, that's why it
  now behaves.
- **Always destroy what you create.** Matterhorn runs production sites and disk is
  not quota-capped per sandbox — the whole reason TTL exists.

---

## How to get a broker to test against

The broker runs on Matterhorn (`ssh -p 2222 root@matterhorn.emorise.com`), image
`buzz-sandbox-broker:local`, source at `/opt/buzz-sandbox-broker`. Rebuild after
changes: **regenerate `Cargo.lock` on the box first** (`docker run --rm -v
"$PWD":/build -w /build rust:1.95-bookworm cargo generate-lockfile`) or the
`--locked` build fails — the box has its own crates tree. Then
`docker build -f Dockerfile.sandbox-broker -t buzz-sandbox-broker:local .` and
recreate the container on all three networks (bridge, dokploy-network,
buzz-sandboxes) — see `buzz-sandbox-viewer-routing` memory for the exact run
command.
