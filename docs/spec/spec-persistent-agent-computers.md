---
title: 'Persistent agent computers'
type: 'feature'
created: '2026-08-29'
status: 'done'
review_loop_iteration: 0
baseline_commit: '648a219d9'
context:
  - VISION_REMOTE_AGENTS.md
  - SANDBOX-PLAN.md
  - HANDOFF-SANDBOX-LIFECYCLE.md
---

<frozen-after-approval reason="human-owned intent - do not modify unless human renegotiates">

## Intent

**Problem:** Agent computers currently store Chromium profiles and files in a disposable Docker container layer. Stopping or expiring a computer removes that layer, so users lose authenticated browser sessions, downloads, and workspace files and must repeat 2FA on the next activation.

**Approach:** Give each agent identity stable, community-scoped Docker volumes for `/home/agent` and `/workspace`, reuse those volumes whenever its computer is recreated, and launch Chromium with its persistent profile and last-session restoration enabled. Compute remains disposable while the PC's user data survives inactivity.

## Boundaries & Constraints

**Always:** Derive volume names deterministically from the community storage namespace and effective sandbox owner, using a hash rather than exposing either value. Keep home and workspace isolated per owner and community. Persist all user-visible paths allowed by the file API. Create volumes explicitly with ownership and lifecycle labels so operators can identify them safely. Preserve Docker hardening, resource limits, network isolation, non-root execution, image allowlisting, and TTL-based compute cleanup. Keep Buzz-owned desktop defaults in the image and refresh only Buzz-owned files without overwriting user-owned files. Let each website enforce its own cookie expiry, so a valid 30-day login remains valid for up to that site's 30 days without Buzz shortening it.

**Ask First:** Adding cross-host or cloud-backed storage, adding a destructive "Reset computer" control, or defining automatic volume-retention/deletion policy.

**Never:** Mount host directories into agent containers, share one browser profile between agents or communities, copy sessions from the user's Mac, migrate already-running legacy computers, automatically delete persistent volumes, keep expired compute running merely to preserve data, log browser/profile contents, or promise a session beyond the website's own expiry and security policy.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|----------------------------|----------------|
| First activation | Owner has no volumes | Docker creates private home and workspace volumes and seeds them from the image | Container creation fails without leaving a running partial sandbox |
| Reactivation | Prior container was destroyed or expired | New container mounts the same volumes; files, cookies, local storage, and browser profile return | Missing/corrupt data follows Chromium and filesystem behavior without falling back to host state |
| Site session expiry | Persisted cookie remains valid or expires | Chromium keeps valid cookies; expired or revoked sessions ask the user to authenticate again | Buzz never fabricates or refreshes website credentials |
| Agent isolation | Two distinct owner pubkeys | Each resolves to different home and workspace volume names | Missing owner falls back to the authenticated caller, never a shared default |
| Browser restart | Chromium or the PC restarts | The same profile opens and Chromium restores the previous session where supported | A damaged profile may open cleanly but is not silently replaced or copied from another machine |
| Legacy computer | PC predates this broker update | It remains disposable; persistence begins after the updated broker creates its replacement | UI/release notes must not claim the legacy PC has durable storage |

</frozen-after-approval>

## Code Map

- `crates/buzz-sandbox-broker/src/sandbox.rs` -- resolves limits and builds the allowlisted, hardened Docker container specification.
- `crates/buzz-sandbox-broker/src/main.rs` -- authenticates create requests, determines the effective owner, creates containers, and reaps expired compute.
- `crates/buzz-sandbox-broker/src/docker.rs` -- creates and labels named volumes, sends the container specification to Docker, and removes containers without deleting named volumes.
- `Dockerfile.sprig-desktop` -- defines the agent home, Chromium profile path, downloads directory, and desktop defaults copied into a fresh home volume.
- `scripts/sprig-desktop-supervise.sh` -- starts and restarts Chromium for the desktop session.
- `crates/buzz-sandbox-broker/src/sandbox.rs` browser launch table -- opens additional browser windows with the same profile.

## Tasks & Acceptance

**Execution:**
- [x] `crates/buzz-sandbox-broker/src/sandbox.rs` -- add stable community-and-owner storage identifiers and named volume mounts for `/home/agent` and `/workspace`; unit-test determinism, isolation, mount targets, and retained hardening.
- [x] `crates/buzz-sandbox-broker/src/main.rs` -- resolve an effective owner and stable storage namespace, create labeled volumes before the container, and use the effective owner consistently for labels, events, deduplication, and storage.
- [x] `crates/buzz-sandbox-broker/src/docker.rs` -- add idempotent labeled-volume creation and retain named volumes when compute containers are removed.
- [x] `scripts/sprig-desktop-supervise.sh` and `crates/buzz-sandbox-broker/src/sandbox.rs` -- add last-session restoration to initial, crash-restart, and manually launched browser paths.
- [x] `Dockerfile.sprig-desktop` and desktop startup scripts -- keep Buzz-owned defaults outside the persistent home, seed or refresh them without overwriting user files, and ensure persistent directories remain owned by UID/GID 10001.

**Acceptance Criteria:**
- Given an agent recreates its PC after stop or TTL expiry, when the new container starts, then `/home/agent` and `/workspace` use the same owner-specific named volumes as before.
- Given a site-issued login is still valid, when the PC is reactivated, then Chromium uses the same cookie store and does not require Buzz-driven reauthentication.
- Given two agents or two community namespaces use the same Docker host, when they create PCs, then neither can receive the other's profile or files through volume naming or fallback behavior.
- Given a PC expires, when the broker reaps it, then CPU and memory are released while its named data volumes remain available for the next activation.
- Given Chromium restarts, when its persistent profile contains a previous session, then every Buzz browser launch path requests restoration without forcing an additional start-page tab.

## Spec Change Log

- Review hardening: required an explicit stable namespace, agent-published owner proof, cross-process create exclusion, graceful Chromium shutdown, non-bind local volumes, lifecycle labels, and symlink-safe default refresh. Kept deterministic owner/community storage and the no-legacy-migration boundary.

## Design Notes

Docker named volumes fit the lifecycle: Docker copies the image's existing `/home/agent` and `/workspace` contents into a new empty volume on first mount, then reuses the volume unchanged on later containers. `DELETE ...?v=true` removes anonymous volumes but retains named volumes. The existing image already places Chromium at `/home/agent/.config/chromium`, so cookies, local storage, IndexedDB, downloads, preferences, and session metadata travel together without exposing host files. Explicit volume labels record the storage namespace, owner hash, and data role without exposing the raw identity.

This guarantees persistence on the same Docker host for computers created after this change. Existing containers are not migrated. Host replacement and backup are storage-provider concerns and require a separately approved durable backend. Persistent volumes consume disk beyond the compute TTL and remain until an operator removes them or a future Reset Computer feature does so explicitly.

## Verification

**Commands:**
- `cargo test -p buzz-sandbox-broker` -- all broker policy, mount, ownership, and routing tests pass.
- `cargo clippy -p buzz-sandbox-broker --all-targets -- -D warnings` -- strict broker lint passes.
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml sandbox_viewer` -- desktop sandbox-viewer tests pass.
- `cargo fmt --all -- --check` -- Rust formatting passes.
- `bash -n scripts/sprig-desktop-supervise.sh scripts/sprig-desktop-entrypoint.sh` -- desktop startup scripts remain valid shell.
- `docker buildx build --check -f Dockerfile.sprig-desktop .` -- Dockerfile validation passes.
- `docker buildx build --load -f Dockerfile.sprig-desktop -t buzz-sprig-desktop:persistence-test .` -- final desktop image builds locally.
- `git diff --check` -- no whitespace errors.

**Manual checks:**
- Local Docker recreation passed: profile, download, and workspace markers survived `docker rm -f -v`, remounted with UID 10001, and remained isolated in explicitly named volumes.
- A live container launched Chromium with `--restore-last-session` from a persisted profile; a 60-second stop delivered the supervisor shutdown path that asks Chromium to flush.
- Managed-default refresh resisted a destination symlink without modifying its unrelated target.
- A production-site login/2FA check remains a deployment smoke test because no production broker or test-site credentials were used during local implementation.

## Suggested Review Order

**Lifecycle and authority**

- Creation binds one authorized agent identity to one stable persistent computer.
  [`main.rs:423`](../../crates/buzz-sandbox-broker/src/main.rs#L423)

- Delegated starts require the agent's signed profile to publish its owner.
  [`identity.rs:267`](../../crates/buzz-sandbox-broker/src/identity.rs#L267)

- Nonterminal Docker states continue reserving the browser profile.
  [`sandbox.rs:929`](../../crates/buzz-sandbox-broker/src/sandbox.rs#L929)

**Persistent storage boundary**

- Hashed namespace and owner IDs define labeled home and workspace volumes.
  [`sandbox.rs:169`](../../crates/buzz-sandbox-broker/src/sandbox.rs#L169)

- Docker validates local, option-free volumes before mounting retained data.
  [`docker.rs:134`](../../crates/buzz-sandbox-broker/src/docker.rs#L134)

- Graceful stop precedes compute deletion while named volumes remain.
  [`docker.rs:194`](../../crates/buzz-sandbox-broker/src/docker.rs#L194)

**Desktop continuity**

- Image-owned defaults seed first activation without owning persistent user data.
  [`Dockerfile.sprig-desktop:152`](../../Dockerfile.sprig-desktop#L152)

- Runtime refresh copies only managed files and refuses symlink redirection.
  [`sprig-desktop-entrypoint.sh:18`](../../scripts/sprig-desktop-entrypoint.sh#L18)

- Chromium restores persistent tabs and flushes its profile on shutdown.
  [`sprig-desktop-supervise.sh:13`](../../scripts/sprig-desktop-supervise.sh#L13)

- Every browser launch path uses the persistent profile and restore request.
  [`sandbox.rs:680`](../../crates/buzz-sandbox-broker/src/sandbox.rs#L680)

**Configuration and proof**

- Deployment must choose a stable namespace instead of inheriting a movable URL.
  [`compose.sandbox-broker.yaml:34`](../../docker/compose.sandbox-broker.yaml#L34)

- Tests pin mount isolation, profile ownership, and hardening together.
  [`sandbox.rs:1144`](../../crates/buzz-sandbox-broker/src/sandbox.rs#L1144)

- Agent-published ownership tests reject unilateral or mismatched claims.
  [`identity.rs:639`](../../crates/buzz-sandbox-broker/src/identity.rs#L639)
