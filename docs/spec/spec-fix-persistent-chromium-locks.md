---
title: 'Recover persisted Chromium profiles after PC shutdown'
type: 'bugfix'
created: '2026-08-30'
status: 'done'
review_loop_iteration: 0
baseline_commit: 'd8f0c91b9ef60da3471c2f08a1922ceb1eb09f76'
context:
  - VISION_REMOTE_AGENTS.md
  - docs/spec/spec-persistent-agent-computers.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** A reactivated Buzz PC can mount its persistent Chromium profile but fail to start Chromium because the previous disposable container left `SingletonLock`, `SingletonSocket`, and `SingletonCookie` artifacts naming its old hostname. The desktop remains reachable through noVNC, but Chromium refuses the profile as active on another computer and the supervisor retries forever.

**Approach:** Treat Chromium's `Singleton*` files as container-scoped runtime state rather than durable profile data. Before each supervised browser launch, remove only those known lock artifacts when no Chromium process is running, while preserving cookies, authentication databases, tabs, preferences, downloads, and every other profile file.

## Boundaries & Constraints

**Always:** Check that neither Chrome nor Chromium is running before cleanup. Remove only the three explicit Chromium singleton artifacts. Make cleanup safe for missing paths, symlinks, regular files, and repeated invocation. Apply recovery to initial reactivation and the existing crash-restart loop. Preserve the broker's one-active-container-per-owner exclusion as the authority preventing concurrent profile use. Log recovery without exposing profile contents.

**Ask First:** Resetting or deleting any wider browser-profile state; changing volume identity or retention; changing the PC shutdown or TTL policy; migrating storage across hosts.

**Never:** Delete cookies, local storage, history, sessions, preferences, downloads, or workspace files. Never clear locks while a browser process is running. Never hide a failed browser launch by reporting the PC ready. Never use the user's local Mac browser profile.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|---------------------------|----------------|
| Reactivation after stale shutdown | Persisted profile has all three `Singleton*` artifacts, no Chromium process | Remove the artifacts and start Chromium with `--restore-last-session` | A cleanup failure is logged and Chromium's own safe refusal remains visible |
| Clean reactivation | Profile has no singleton artifacts | Launch normally without changing profile data | Missing artifacts are success |
| Live browser | Chrome or Chromium process exists | Leave all singleton artifacts untouched | Refuse cleanup rather than risk concurrent profile access |
| Crash-restart | Browser died and left locks in the current container | Watchdog clears the artifacts before retrying | Repeated cleanup remains idempotent |
| Unexpected artifact type | A named singleton path is a symlink or regular file | Remove that exact directory entry without following it | A directory or removal error is logged and does not broaden deletion |

</frozen-after-approval>

## Code Map

- `scripts/sprig-desktop-supervise.sh` -- owns Chromium startup, shutdown, readiness polling, and crash retries.
- `scripts/sprig-desktop-browser.sh` -- serializes every browser entry point across cleanup and process startup.
- `scripts/sprig-desktop-browser.desktop` and `scripts/sprig-desktop-tint2rc` -- keep the in-screen dock on the serialized browser launcher.
- `scripts/sprig-desktop-profile-locks.sh` -- proposed focused, sourceable cleanup helper with the browser-running safety guard.
- `scripts/test-sprig-desktop-profile-locks.sh` -- proposed deterministic shell regression coverage for stale, missing, live-browser, and unrelated-profile states.
- `Justfile` -- runs the focused shell regression through the repository's normal unit/CI gate.
- `Dockerfile.sprig-desktop` -- installs the runtime helper in the desktop PC image.
- `crates/buzz-sandbox-broker/src/sandbox.rs` -- retains the existing persistent-volume and single-active-owner invariants; no behavioral change expected.

## Tasks & Acceptance

**Execution:**
- [x] `scripts/sprig-desktop-profile-locks.sh` -- add idempotent, narrowly scoped lock recovery guarded by local browser-process detection.
- [x] `scripts/sprig-desktop-supervise.sh` and `Dockerfile.sprig-desktop` -- invoke and package recovery for every supervised launch without changing persistent-session arguments.
- [x] `scripts/test-sprig-desktop-profile-locks.sh` -- prove stale locks are removed, unrelated profile data survives, missing locks are harmless, and a live browser prevents cleanup.
- [x] Matterhorn deployment -- rebuild the desktop PC image, reactivate the existing persisted PC, and exercise a real stop/recreate cycle.

**Acceptance Criteria:**
- Given the currently affected persistent profile points to old container `e9fd959ba254`, when the fixed image creates a new PC, then Chromium starts, CDP port 9222 responds, and the browser window is visible through the existing noVNC screen.
- Given that fixed PC is stopped and recreated again with a new container hostname, when Chromium restores the same profile, then it starts without a ProcessSingleton error and retained session/profile data remains present.

## Spec Change Log

## Design Notes

The helper is separated from the supervisor so its destructive scope can be tested directly without starting Xvfb or Chromium. Removing a symlink with `rm` removes the link itself, not its target. The broker already prevents two nonterminal containers for one owner from mounting the same profile; the process check adds a local defense before cleanup.

## Verification

**Commands:**
- `bash scripts/test-sprig-desktop-profile-locks.sh` -- all lock-state cases pass.
- `bash -n scripts/sprig-desktop-profile-locks.sh scripts/sprig-desktop-supervise.sh scripts/test-sprig-desktop-profile-locks.sh` -- shell syntax passes.
- `cargo test -p buzz-sandbox-broker` -- persistence/lifecycle policy remains green.
- `docker buildx build --check -f Dockerfile.sprig-desktop .` -- image definition remains valid.
- `git diff --check` -- no malformed patch content.

**Manual checks:**
- Matterhorn image `sha256:c54a4b1fe9937312f41df5ae3e973d4a7751a6265832dcaa21ce5d47bf22c3b7` passed live reactivation: Chromium ran, `/json/version` on port 9222 succeeded, and `SingletonLock` named the new container.
- A graceful stop and second recreation under a different hostname also reached CDP, produced no ProcessSingleton error, and retained temporary markers in both `/home/agent` and `/workspace`. The markers and both smoke containers were removed afterward; the two persistent volumes remain.

## Suggested Review Order

**Serialized browser startup**

- Centralize every browser entry point behind cleanup and a short cross-process launch lock.
  [`sprig-desktop-browser.sh:15`](../../scripts/sprig-desktop-browser.sh#L15)

- Route the visible dock through the same guarded launcher.
  [`sprig-desktop-tint2rc:200`](../../scripts/sprig-desktop-tint2rc#L200)

- Preserve normal desktop-file behavior while selecting the guarded command.
  [`sprig-desktop-browser.desktop:1`](../../scripts/sprig-desktop-browser.desktop#L1)

**Narrow state recovery**

- Fail closed on process-inspection errors and unsafe profile paths.
  [`sprig-desktop-profile-locks.sh:10`](../../scripts/sprig-desktop-profile-locks.sh#L10)

- Remove only Chromium's three transient singleton entries without following links.
  [`sprig-desktop-profile-locks.sh:55`](../../scripts/sprig-desktop-profile-locks.sh#L55)

- Keep restored sessions and watchdog retries on the guarded launcher.
  [`sprig-desktop-supervise.sh:229`](../../scripts/sprig-desktop-supervise.sh#L229)

**Packaging and regression proof**

- Install recovery and launcher assets into image-owned managed defaults.
  [`Dockerfile.sprig-desktop:176`](../../Dockerfile.sprig-desktop#L176)

- Exercise stale, live, malformed, and unrelated-profile states deterministically.
  [`test-sprig-desktop-profile-locks.sh:48`](../../scripts/test-sprig-desktop-profile-locks.sh#L48)

- Run the regression automatically through the repository unit gate.
  [`Justfile:313`](../../Justfile#L313)
