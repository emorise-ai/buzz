# Handoff — Teach a Task (and disk persistence)

**Written 2026-08-18.** The agent-computer surface is now a real, polished
product: an agent can see and drive its own Linux desktop, a human can watch it
in a sidebar preview or a pop-out window, computers stay alive while in use and
reconnect on restart, the desktop is a native tint2 dock with Buzz branding, and
the Computer icon persists per agent. All of that is built, CI-green, deployed to
staging, and committed on `feat/agent-sandboxes` (30 commits ahead of `main`).

**Two things remain unbuilt.** This file briefs both. The headline one is
**Teach a Task** — currently a disabled "Coming soon" button in
`desktop/src/features/agents/sandbox/SandboxViewerDialog.tsx:154`. The structural
one is **true disk persistence**. They are related: a taught task worth repeating
usually assumes the computer's files/logins persist, so persistence is a soft
prerequisite for teaching to feel real.

Read alongside: `HANDOFF-BUZZ-COMPUTER.md` (the workspace), `HANDOFF-SANDBOX-
LIFECYCLE.md` (create/extend/destroy), `SANDBOX-PLAN.md` (the infra phases, all
done), and the memory note `buzz-computer-use-surface` (the running record of
what shipped, with commit hashes and gotchas).

---

## What "Teach a Task" is

The product promise: **you demonstrate a task once on the agent's computer, and
it learns to repeat it.** You do the thing (log into a portal, download a report,
rename and file it), the agent watches, and afterward it can do that same task on
demand — and eventually on a schedule (that's the "skills/routines" follow-on).

This is the feature that makes the computer more than a remote desktop. Everything
built so far is the *substrate* for it: the agent has hands (exec/click/type),
eyes (screenshot), a place to work (the sandbox), and the human can drive the same
screen (takeover). Teaching is the layer that captures a human demonstration and
turns it into something the agent can replay.

---

## The shape of the work (design, not yet code)

There is no committed design for this beyond the "Coming soon" button and the
product artifact. Here is the honest decomposition; each bullet is a real decision
or build, roughly in dependency order.

### 1. Capture a demonstration
When the human clicks "Teach a task" and performs actions on the computer, we need
to record what they did. Two very different fidelities:

- **Semantic capture (recommended to start):** record the meaningful steps, not raw
  pixels — "navigated to X", "clicked the Login button", "typed into the Email
  field", "downloaded file Y". The broker already sees input events
  (`POST /sandboxes/{id}/input`) and can observe the screen; the taught trace is a
  structured list of steps + periodic screenshots as anchors. This is what an
  agent can actually *reason about and replay* (it can re-find "the Login button"
  even if pixels shift), and it's what a language model turns into a repeatable
  routine.
- **Literal capture (a screen recording):** a video of the session. Good for the
  human to review, useless for robust replay (pixel coordinates break the moment
  anything moves). At most a companion artifact, not the source of truth.

**Where capture happens:** the human drives via the desktop viewer (a noVNC
WebSocket) and/or the broker input API. To capture semantic steps you either (a)
route the human's inputs through the broker's `/input` endpoint so the broker logs
them, or (b) instrument inside the container. Today human takeover drives the
screen directly through noVNC — the broker does NOT see those individual events
(it's one WebSocket). So **capturing a human demo needs a new path**: either the
desktop app sends the human's actions through `/input` (and mirrors them to the
screen) during a teaching session, or an in-container recorder watches X events.
This is the first real design fork — decide it early.

### 2. Turn the demonstration into a routine
A raw trace isn't a skill. An LLM pass distills the steps into a named,
parameterized routine: "Download the weekly sales report" with a summary, the
ordered steps, and any inputs that should be variables (date range, which report).
This is a natural fit for the existing agent harness (`buzz-acp` / the agent).
The routine is stored as a Nostr event (new kind in `buzz-core/src/kind.rs`,
`48xxx` range like the other sandbox events) so it syncs and shows up in the UI
like everything else — do NOT invent a new HTTP store; model it as an event.

### 3. Replay
The agent executes a stored routine on its computer using the tools it already
has (`buzz sandbox exec/click/type/screenshot/open`). Replay must be
**adaptive, not literal** — re-find elements by what they are, screenshot to
verify each step landed, recover or ask for help when the screen doesn't match.
The agent's existing "look before and after acting" briefing already points this
way. Replay is where semantic capture pays off and literal capture fails.

### 4. Skills / routines library (the follow-on)
Taught tasks become reusable, listable, runnable-on-demand, and eventually
**schedulable** (a routine that runs every Monday). The `buzz-workflow` crate
(YAML-as-code workflow engine, evalexpr conditions) is the natural home for
scheduling and conditions once a routine exists. The UI: the disabled button
becomes "Teach a task", and there's a place to see/run/schedule saved skills.

---

## Product decisions still open (ask the user per phase, not all at once)

From the original plan artifact, these were parked and are still unanswered:

- **D1 — disk retention:** how long is a computer's disk kept after it stops?
  (Blocks persistence; see below.)
- **D2 — teaching transcription/recording:** is a teaching session recorded
  (video and/or transcript)? This is a **privacy decision** — a demo may include
  the human typing a password into a real site. Default to semantic capture with
  secrets redacted; get explicit consent before storing any recording.
- **D3 — browser-first vs whole-desktop teaching:** can you teach any desktop
  task, or only in-browser flows to start? Browser-first is dramatically simpler
  (DOM gives stable element identity; no pixel-hunting) and covers most real
  "download the report / fill the form" tasks. **Strong recommendation: ship
  browser-first teaching, expand to whole-desktop later.**
- **D4 — recording privacy:** where taught traces/recordings live, who can see
  them, retention. Tied to D2.
- **D5 — the terminal-vs-computer rule:** when should a taught task use the shell
  (deterministic, robust) vs the GUI (what the human demonstrated)? Prefer shell
  where a shell can do the job; GUI only when it must.

---

## True disk persistence (the structural prerequisite)

Fully scoped already — repeated here so this handoff stands alone. Today a
sandbox is 100% ephemeral: `container_spec` in
`crates/buzz-sandbox-broker/src/sandbox.rs` sets `Binds: []`, and the reaper
`remove_container`s with `v=true` (deletes anonymous volumes). Stop = total data
loss. To make files survive:

1. **Volume per owner:** mount a named Docker volume
   (`buzz-sandbox-home-<owner_pubkey>`) at `/workspace` in `container_spec`
   (currently `Binds: []`). Needs NEW volume API in `docker.rs` (create/inspect
   volume — none exist today). Keyed by `req.owner` (= agent pubkey, already the
   `LABEL_OWNER` value). On create, reuse the owner's existing volume if present.
2. **The hard part — disk GC:** volumes surviving removal accumulate forever, and
   disk is NOT quota-capped on the Matterhorn host (that's the whole reason the
   session TTL reaper is aggressive). Persistence MUST add a **second, longer
   disk-retention lifecycle**: a separate reaper listing volumes by a
   `com.buzz.sandbox.volume-owner` label and removing ones idle past a much longer
   window (days/weeks). Without this, persistence reopens the unbounded-disk risk
   the TTL exists to close. This is net-new design, not a flag flip.
3. Decisions: how long disks are kept (D1), per-owner disk cap, what "reset my
   computer" does.

**Estimate:** a multi-session project on its own. It should probably land *before*
or alongside Teach-a-Task, because a taught task that logs into a site expects
that login (a cookie/session on disk) to still be there next run.

---

## Sequencing recommendation

1. **Disk persistence first** (volume-per-owner + disk-GC). It's the structural
   piece and it makes taught tasks actually repeatable across sessions.
2. **Browser-first Teach-a-Task**, semantic capture: instrument the teaching
   session to capture steps, LLM-distill into a routine event, adaptive replay.
   Start narrow (browser flows) where element identity is stable.
3. **Skills/routines library + scheduling** (via `buzz-workflow`).
4. **Whole-desktop teaching** later, if browser-first proves the model.

Each is independently shippable and each answers its own parked product question
when its phase starts — surface D1 with persistence, D2/D3/D4 with teaching, D5
with replay. Don't front-load all five on the user.

---

## State to build on (all committed on `feat/agent-sandboxes`)

- Broker computer-use: `POST /sandboxes/{id}/exec|input`, `GET .../screenshot`,
  `/launch` (returns JSON), `/heartbeat`, owner-or-manager gate, activity keepalive
  middleware, per-owner create dedupe. `crates/buzz-sandbox-broker`.
- CLI: `buzz sandbox exec/screenshot/click/type/key/scroll/open/create/list/
  status/extend/destroy`, id self-detection. `crates/buzz-cli`.
- Desktop: sidebar preview + pop-out window (shared `SandboxStage`), attach-agent-
  to-computer on "Start a computer" (restart wires `BUZZ_SANDBOX_ID`), viewer
  heartbeat, reconnect-liveness probe, persistent computer icon + message
  indicator. `desktop/src/features/agents/sandbox`, `desktop/src-tauri`.
- Image: `Dockerfile.sprig-desktop` — Xvfb+openbox+picom, tint2 dock, Yaru icons,
  Buzz wallpaper + branded start page, `scrot`, `xdotool`.
- The "Teach a task" button lives in `SandboxViewerDialog.tsx:154` (disabled).
  That's the entry point to wire up.

## Deploy + test facts (do not relearn)

- Broker on Matterhorn: `/opt/buzz-sandbox-broker`, image `buzz-sandbox-broker:
  local`. Rebuild: rsync crate + Cargo.lock → `generate-lockfile` in a rust
  container → `docker build -f Dockerfile.sandbox-broker` → recreate container on
  3 nets (bridge/buzz-sandboxes/dokploy-network) + `-p 127.0.0.1:9310:9310` +
  docker.sock. Broker restart is SAFE (reseeds expiry from relay 48200).
- Desktop image `buzz-sprig-desktop:latest` must be rebuilt separately for image
  changes AND is auto-pruned by Dokploy — keep it (and `buzz-sprig-dev:latest`)
  pinned via the `buzz-image-pin-*` holder containers, or it 404s.
- Test the app against `wss://relay.staging.emorise.com` via `just
  desktop-standalone` (NOT `just dev`, which forces a local relay with no broker).
- Membership is open on staging, so any signer is admitted for CLI testing over an
  `ssh -L 9310` tunnel signing against `http://127.0.0.1:9310`.
- Always destroy test sandboxes; Matterhorn runs production sites.
