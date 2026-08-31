---
title: 'Stop healthy Onshape sessions from restart-looping'
type: 'bugfix'
created: '2026-08-31'
status: 'done'
review_loop_iteration: 0
baseline_commit: 'b30131c1def6c6673274f5fd74e3e82e801e5155'
context:
  - VISION_REMOTE_AGENTS.md
  - docs/spec/spec-fix-onshape-webgl.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The live Buzz PC repeatedly appears to crash while Onshape is open. Matterhorn evidence shows Chromium's Mesa WebGL renderer remains healthy, but the supervisor treats a two-second page-probe timeout as renderer failure and deliberately restarts Chromium about every 30 seconds under sustained two-core CAD load.

**Approach:** Separate confirmed renderer failure from an inconclusive page probe. Keep strict WebGL validation at startup and continue recovering real GPU crashes, but never restart a live browser solely because a busy page did not answer CDP before the probe deadline.

## Boundaries & Constraints

**Always:** Preserve the persistent Chromium profile, Onshape login, tabs, downloads, and workspace. Keep browser-process liveness and browser-wide GPU crash state observable. Require a successful WebGL2 and extension probe before startup readiness. Record inconclusive runtime checks without counting them as failures.

**Ask First:** Expanding PC CPU or memory limits, enabling Matterhorn's Intel GPU, rebooting Matterhorn, or changing the broker's security boundary.

**Never:** Restart Chromium because a page-level CDP request merely timed out; disable renderer monitoring; reset the browser profile; weaken recovery for a confirmed GPU-process crash, disabled WebGL state, or exited browser.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|---------------------------|----------------|
| Busy Onshape document | Browser-wide GPU state is healthy; page probe times out | Browser and tabs stay alive; health is logged as inconclusive | Reset the runtime failure counter and retry at the next interval |
| Confirmed renderer failure | GPU crash count is nonzero or WebGL is disabled | Chromium restarts gracefully after the existing debounce | Restore the same persistent profile and require startup health again |
| Browser exit | No live Chromium process | Supervisor relaunches Chromium | Restore the persistent session and wait for WebGL readiness |
| Initial startup under load | Page probe is temporarily inconclusive | Do not claim readiness until a full check succeeds | Retry within the bounded startup window; avoid a destructive restart for inconclusive status |

</frozen-after-approval>

## Code Map

- `scripts/sprig-desktop-webgl-health.mjs` -- classifies browser-wide GPU failure, successful page WebGL validation, and inconclusive page communication.
- `scripts/sprig-desktop-webgl-policy.sh` -- pure, executable mapping from health statuses and debounce state to supervisor actions.
- `scripts/sprig-desktop-supervise.sh` -- converts the health command's distinct exit statuses into readiness, deferred checks, or confirmed-failure recovery.
- `Dockerfile.sprig-desktop` -- packages the policy helper beside the supervisor and health command.
- `scripts/test-sprig-desktop-webgl-health.mjs` -- covers classification of busy-page timeouts without weakening confirmed-failure detection.
- `scripts/test-sprig-desktop-webgl-config.sh` -- checks supervisor wiring for the inconclusive status.

## Tasks & Acceptance

**Execution:**
- [x] `scripts/sprig-desktop-webgl-health.mjs` -- return a distinct inconclusive status when browser-wide GPU state is healthy but page-level CDP cannot answer; retain failure status for malformed or explicitly unhealthy renderer state.
- [x] `scripts/sprig-desktop-supervise.sh` -- preserve the health command's exit code and defer runtime recovery for inconclusive checks without reporting readiness.
- [x] `scripts/test-sprig-desktop-webgl-health.mjs` and `scripts/test-sprig-desktop-webgl-config.sh` -- prove timeouts do not restart Chromium while confirmed GPU failures still do.
- [x] Matterhorn candidate image -- run the persisted Grill Module beyond several health intervals and prove the Chromium PID, WebGL renderer, and Onshape session remain stable.

**Acceptance Criteria:**
- Given the persisted Grill Module consumes both assigned CPU cores, when at least four runtime health intervals pass, then Chromium keeps the same PID and no supervisor restart is logged.
- Given a synthetic GPU crash, when the runtime monitor observes confirmed unhealthy state twice, then Chromium restarts gracefully and returns with WebGL2 healthy.
- Given the PC is stopped and recreated, when Onshape reopens, then the existing authenticated session and files remain present.

## Spec Change Log

## Design Notes

The health command should use three outcomes: healthy, unhealthy, and inconclusive. Browser-level `SystemInfo.getInfo` can confirm the renderer backend, WebGL feature state, and GPU crash count without scheduling work on Onshape's busy main thread. Once that browser-wide check passes, a timeout while creating the isolated page probe says nothing about renderer health. An explicit page response that lacks WebGL2 or required extensions remains a real failure.

## Verification

**Commands:**
- `node --test scripts/test-sprig-desktop-webgl-health.mjs` -- 25 tests pass, including busy-page timeout exit 2, page navigation/exception exit 2, browser-wide timeout exit 1, malformed page state exit 1, and confirmed GPU crash exit 1 before any page probe.
- `bash scripts/test-sprig-desktop-webgl-config.sh` -- passes; the executable policy restarts only after consecutive status-1 results and resets on healthy, inconclusive, unavailable, and unknown process exits.
- `bash -n scripts/sprig-desktop-supervise.sh` -- passes.
- `pnpm exec biome check scripts/sprig-desktop-webgl-health.mjs scripts/test-sprig-desktop-webgl-health.mjs` -- passes.
- `git diff --check` -- passes.

**Matterhorn evidence (2026-08-31):**
- Candidate image `sha256:91091ce59649...` reused the existing named home/workspace volumes under the production two-CPU, 4 GiB boundary.
- The persisted `TLX Modules | Grill Module` stayed open for five 15-second monitoring intervals at about 189% CPU. Chromium kept PID 63. The last two page probes timed out and returned inconclusive status 2, while browser-wide CDP continued to report ANGLE/OpenGL, Mesa llvmpipe, WebGL enabled, and zero GPU crashes. The supervisor logged no renderer restart.
- A separate candidate container killed GPU PID 112. The supervisor recorded confirmed failures 1/2 and 2/2, gracefully replaced Chromium PID 50 with PID 558 after 28 seconds, and returned to WebGL2 healthy with zero crashes.
- After stopping and recreating the candidate with the same named volumes, Chromium retained 478 profile files and opened Onshape directly to `Owned by me | Documents`, proving the authenticated session survived without 2FA. A temporary workspace marker also survived a separate container recreation and was removed after verification.

**Manual checks (if no CLI):**
- Keep the persisted Onshape Grill Module visible for at least one minute and confirm the 3D viewport remains usable without a browser restart.

## Suggested Review Order

**Health classification**

- Establish browser-wide GPU health before treating page failures as inconclusive.
  [`sprig-desktop-webgl-health.mjs:311`](../../scripts/sprig-desktop-webgl-health.mjs#L311)

- Map only confirmed consecutive failures to a destructive restart.
  [`sprig-desktop-webgl-policy.sh:6`](../../scripts/sprig-desktop-webgl-policy.sh#L6)

**Supervisor behavior**

- Preserve health exit status and normalize unknown process failures safely.
  [`sprig-desktop-supervise.sh:298`](../../scripts/sprig-desktop-supervise.sh#L298)

- Apply the pure policy without restarting on inconclusive CAD probes.
  [`sprig-desktop-supervise.sh:383`](../../scripts/sprig-desktop-supervise.sh#L383)

**Regression proof**

- Exercise timeout, navigation, exception, malformed-state, and GPU-crash classifications.
  [`test-sprig-desktop-webgl-health.mjs:470`](../../scripts/test-sprig-desktop-webgl-health.mjs#L470)

- Execute the restart policy across healthy, failure, inconclusive, and unknown statuses.
  [`test-sprig-desktop-webgl-config.sh:73`](../../scripts/test-sprig-desktop-webgl-config.sh#L73)

- Package the policy helper beside the desktop supervisor.
  [`Dockerfile.sprig-desktop:182`](../../Dockerfile.sprig-desktop#L182)
