# Handoff — show an agent's sandbox in the Buzz desktop

**Written 2026-08-16.** Answers a direct question: *is there a preview and full
view of the remote screen in the desktop yet?*

**No. Nothing in the desktop reads sandbox events.** The backend is finished and
proven; the UI does not exist. This file is the brief for building it.

Read `SANDBOX-ARCHITECTURE-OPTIONS.md` for why sandboxes are shaped the way they
are, and `MANUAL-PASS-FINDINGS.md` for what has been verified live.

---

## What already exists

Everything the UI needs is on the relay today. A sandbox announces itself.

**kind:48200 — sandbox created** (`buzz-core/src/kind.rs:604`)

| tag | meaning |
|---|---|
| `d` | sandbox id — the addressable key |
| `name` | container name, e.g. `buzz-sandbox-fq5ijxh7lt` |
| `image` | which image is running |
| `cpus`, `memory_mb` | the enforced budget |
| `expires_at` | unix seconds; the reaper destroys it then |
| `p` | the agent that owns it — **this is what a card filters on** |
| `viewer` | URL of the live desktop, when one is exposed |

**kind:48201 — sandbox destroyed** carries `d` and a `reason`
(`expired` or `destroyed`).

Both kinds are accepted by the relay and published by the broker; that was
verified live, not assumed.

---

## What is missing

1. **No subscription.** No desktop code references 48200/48201. Grep confirms
   it: the kinds exist in `buzz-core` and appear nowhere under `desktop/src`.
2. **No card.** An agent with a computer looks identical to one without.
3. **No viewer.** The sandbox image serves a full noVNC client on port 6080
   (`Dockerfile.sprig-desktop`), but nothing embeds it.
4. **`viewer` is usually empty.** The broker only sets it when
   `BUZZ_SANDBOX_VIEWER_URL` is configured, and on Matterhorn it is not — so
   today's events carry no link. **Fix this first**; a UI built against events
   that lack the field will be built blind.

---

## The job

### 1. Make the viewer reachable (do this before any UI work)

The sandbox's noVNC server is on its own bridge network with no published port.
Something must route to it. Cheapest correct option is a path on the existing
Traefik instance, e.g. `https://sandbox.staging.emorise.com/<id>/`, then set
`BUZZ_SANDBOX_VIEWER_URL` on the broker so events carry a working link.

Until that exists, opening a sandbox desktop means an SSH tunnel to port 6080,
which is fine for development and useless as a product.

**Security note, non-optional:** that URL is a live desktop with a browser the
agent has logged into. It must not be publicly reachable. Whatever fronts it
needs authentication — the relay's own session, or Traefik with an allowlist.
Do not publish an unauthenticated viewer URL into an event that any community
member can read.

### 2. Preview — "this agent has a computer"

On the agent card in `desktop/src/features/agents/ui/`:

- Subscribe to kind:48200 filtered by `#p` = the agent's pubkey.
- Show image, CPU/memory, and a live countdown to `expires_at`.
- Remove it on kind:48201 for the same `d`.
- A small still frame is nicer than a placeholder. `browser_screenshot` already
  exists as an MCP tool; a periodic thumbnail is a later refinement, not v1.

### 3. Full view

Open the `viewer` URL in a panel. noVNC is a complete browser client, so an
iframe is enough — there is no custom rendering to write.

Two things that matter:

- **Say the human is in control.** The agent and the person share one browser;
  that is the point of the design. The UI should make taking the mouse feel
  deliberate, not accidental.
- **Handle expiry.** A sandbox can be reaped while the panel is open. Watch for
  48201 and replace the frame with something that explains it, rather than
  leaving a dead iframe.

---

## Conventions to follow

- Kinds are mirrored in `desktop/src/shared/constants/kinds.ts` — add them
  there rather than inlining integers.
- Text sizing is rem-only; a CI guard (`pnpm check:px-text`) fails on px
  literals. See `CLAUDE.md` § Text sizing.
- **Any module-level cache you add must be reset in `resetCommunityState()`**
  (`desktop/src/features/communities/useCommunityInit.ts`), or sandbox state
  from one community leaks into the next.
- Screenshots for the PR: `just desktop-screenshot`, then
  `scripts/post-screenshots.sh`. Never link relay media URLs — they fail
  through GitHub's camo proxy.

---

## How to get a sandbox to look at

The broker runs on Matterhorn (`ssh -p 2222 root@matterhorn.emorise.com`).
Tunnel it on the **same** port — NIP-98 signs the exact URL, so a different
local port fails verification in a way that looks like a broken signature:

```bash
ssh -p 2222 -L 9310:127.0.0.1:9310 root@matterhorn.emorise.com
```

Create one with a signed request (see `MANUAL-PASS-FINDINGS.md` for the
mechanics). The response carries `tools_url` and `tools_token`. Then watch the
events land:

```bash
buzz --format compact messages search --kinds 48200,48201 --limit 10
```

**Always destroy what you create.** Matterhorn runs production sites, and disk
is not quota-capped per sandbox.

---

## Sequencing

1. Route and authenticate the viewer; set `BUZZ_SANDBOX_VIEWER_URL`.
2. Preview card, driven by real events.
3. Full view panel.

Step 1 is infrastructure and gates the rest. Steps 2 and 3 are ordinary desktop
feature work against events that already exist and already flow.
