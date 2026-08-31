---
title: 'Keep Onshape WebGL alive in Buzz PCs'
type: 'bugfix'
created: '2026-08-31'
status: 'done'
review_loop_iteration: 1
baseline_commit: 'a42a356c31d09498a881e2d5f08cbfa38b91d713'
context:
  - VISION_REMOTE_AGENTS.md
  - docs/spec/spec-persistent-agent-computers.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Chromium 151 initially gives Onshape a SwiftShader-backed WebGL context, but the GPU process crashes under the Grill Module workload. After two crashes Chromium disables WebGL for the session, and Onshape reports that its rendering context was lost.

**Approach:** Replace Chromium's unstable SwiftShader/Vulkan fallback with ANGLE over Mesa OpenGL/llvmpipe, which remains software-rendered inside the existing hardened container but survived the representative WebGL2 stress probe. Apply the renderer policy at the shared browser-launch boundary so supervised, dock, and broker launches cannot drift.

## Boundaries & Constraints

**Always:** Preserve the persistent Chromium profile, restored tabs, cookies, downloads, and workspace files. Keep the container's capability drop, no-new-privileges policy, memory/CPU limits, network boundary, and lack of host mounts/devices. Use one renderer policy for every browser entry point. Retain a functional WebGL2 context and Onshape-required float framebuffer/texture extensions.

**Ask First:** Enabling Matterhorn's blacklisted Intel iGPU, changing its bootloader, rebooting the host, passing `/dev/dri` into containers, or expanding resource limits for all PCs.

**Never:** Reset the user's browser profile; disable WebGL; use the local Mac browser or credentials; add privileged containers or host GPU access as an unreviewed workaround; claim success from process liveness alone when CDP reports WebGL unavailable.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|---------------------------|----------------|
| Fresh or restored PC | Browser starts through supervisor | Chromium uses ANGLE/OpenGL with Mesa llvmpipe and WebGL2 is available | Startup logs expose renderer initialization failures |
| Alternate launch | Dock or broker opens Chromium during a restart gap | Shared wrapper applies the same renderer arguments | No unconfigured browser can become the surviving profile owner |
| Onshape model load | Persistent Onshape session opens a 3D document | WebGL context remains live and the model viewport renders | GPU-process crash/context loss fails live acceptance and blocks rollout completion |
| Non-WebGL browsing | Ordinary sites and browser chrome | Continue to render normally with the same profile/session | Existing browser readiness and recovery remain intact |

</frozen-after-approval>

## Code Map

- `Dockerfile.sprig-desktop` -- installs Chromium and the Mesa EGL/GLES runtime needed by ANGLE/OpenGL.
- `scripts/sprig-desktop-browser.sh` -- serialized browser entry point shared by supervisor and human launchers; owns renderer arguments.
- `scripts/sprig-desktop-supervise.sh` -- launches/restores Chromium and documents the renderer policy delegated to the wrapper.
- `scripts/sprig-desktop-webgl-health.mjs` -- queries Chromium through CDP and refuses renderer states where WebGL is unavailable or the GPU process has crashed.
- `scripts/test-sprig-desktop-webgl-config.sh` -- focused launch-argument and image-package regression coverage.
- `scripts/test-sprig-desktop-webgl-health.mjs` -- deterministic CDP health-state coverage.
- `Justfile` -- runs the focused regression in the normal unit gate.

## Tasks & Acceptance

**Execution:**
- [x] `Dockerfile.sprig-desktop` -- install Mesa EGL/GLES libraries required for the proven llvmpipe path.
- [x] `scripts/sprig-desktop-browser.sh` and `scripts/sprig-desktop-supervise.sh` -- centralize ANGLE/OpenGL renderer defaults, discard caller flags that can disable or replace WebGL, and remove SwiftShader-specific flags from every launch.
- [x] `scripts/sprig-desktop-webgl-health.mjs` and `scripts/sprig-desktop-supervise.sh` -- require CDP to report WebGL enabled with no GPU-process crashes before logging renderer readiness, and recover a live browser whose GPU subprocess later becomes unhealthy.
- [x] `scripts/test-sprig-desktop-webgl-config.sh`, `scripts/test-sprig-desktop-webgl-health.mjs`, and `Justfile` -- prove all launch paths inherit the exact renderer policy, conflicting flags are rejected, health-state branches behave correctly, and the image contains its runtime dependencies.
- [x] Candidate PC image -- assemble and exercise the Mesa renderer through CDP before release; keep the reproducible full Dockerfile build as a release check when the local builder lacks space.

**Acceptance Criteria:**
- Given a clean image probe, when Chromium starts, then CDP `SystemInfo.getInfo` reports `ANGLE_OPENGL`, Mesa llvmpipe, WebGL enabled, and zero GPU-process crashes after the stress workload.
- Given a persisted browser profile, when the candidate image reuses it across a stop/recreate cycle, then Chromium restores the session without clearing profile or workspace data and CDP still reports WebGL enabled.
- Given Chromium's main process remains alive after a GPU-process crash or WebGL disablement, when the supervisor's health interval runs, then it does not report renderer readiness and gracefully restarts Chromium through the shared wrapper.

## Spec Change Log

- Review loop 1: the first patch selected and proved the stable Mesa renderer but treated `/json/version` and main-process liveness as sufficient readiness. Added an explicit CDP WebGL/GPU-process health task, ongoing recovery, conflicting-disable-flag handling, and branch tests so the frozen “never claim success from process liveness alone” rule is enforceable. KEEP: ANGLE/OpenGL over Mesa llvmpipe, one shared wrapper policy, no host GPU/device changes, persistent-profile preservation, and the successful stress plus stop/recreate proof.
- Review patch pass 2: strengthened readiness with an isolated page-target WebGL2 context probe and both Onshape-required float extensions. Bounded the complete multi-stage CDP probe and startup window, required two consecutive runtime failures before recovery, made a missing health runtime explicitly defer recovery without claiming readiness, and normalized duplicate profile plus valued disable arguments.

## Design Notes

Live evidence on 2026-08-31 showed two SwiftShader GPU-process exits (`exit_code=512`) followed by `webgl: unavailable_software`. An isolated copy of the same Chromium build with `libegl-mesa0`, `libgles2`, and `--use-angle=gl` reported ANGLE/OpenGL over llvmpipe, all relevant WebGL features enabled, and completed 1,510 large-texture render/readback frames without context loss. The broker's 64 MiB `/dev/shm` is not changed because Debian Chromium already injects `--disable-dev-shm-usage`, and the evidence points to the renderer crash rather than shared-memory exhaustion.

## Verification

**Commands:**
- `bash scripts/test-sprig-desktop-webgl-config.sh` -- renderer arguments and image dependencies pass.
- `node --test scripts/test-sprig-desktop-webgl-health.mjs` -- 18 GPU, page-WebGL2, extension, context-loss, HTTP, websocket, timeout, malformed-response, and CDP-error checks pass.
- `bash -n scripts/sprig-desktop-browser.sh scripts/sprig-desktop-supervise.sh scripts/test-sprig-desktop-webgl-config.sh` -- shell syntax passes.
- `just test-unit` -- focused regression remains wired into the repository gate.
- `docker buildx build --check -f Dockerfile.sprig-desktop .` -- image definition validates.
- `docker build -f Dockerfile.sprig-desktop -t buzz-sprig-desktop:webgl-test .` -- image builds for runtime proof.
- `git diff --check` -- patch is clean.

**Manual checks (if no CLI):**
- In the reactivated persisted PC, visually confirm the Onshape Grill Module viewport is rendered and interactive while CDP reports a live WebGL2 context and no new GPU-process crashes.

**Local evidence (2026-08-31):**
- The focused configuration regression and shell syntax checks pass, and the Dockerfile build check reports no warnings.
- A temporary candidate assembled from the last locally built desktop image, the declared Mesa packages, and the updated browser wrapper ran under the broker's capability drop, no-new-privileges policy, 4 GiB memory limit, and 2 CPU limit. CDP reported `ANGLE_OPENGL`, Mesa llvmpipe, `webgl: enabled`, and `processCrashCount: 0`.
- A WebGL2 probe allocated a 4096-by-4096 framebuffer and completed 1,510 render/readback frames with zero context-loss events or GL errors. `EXT_color_buffer_float` and `OES_texture_float_linear` were both available.
- After graceful browser shutdown and container recreation, the same named home and workspace volumes retained their markers and Chromium `Local State`; the wrapper restored the profile with the same Mesa arguments and CDP again reported WebGL2 live with zero GPU-process crashes.
- Review-loop candidate fault injection killed Chromium's GPU subprocess while its main process remained alive. The CDP check observed `processCrashCount: 1`; the supervisor did not claim readiness, gracefully replaced the main browser process through `buzz-browser`, restored the same persistent profile, and returned to `ANGLE_OPENGL`/llvmpipe with WebGL enabled and zero crashes. Home and workspace markers remained intact.
- Container PID 1 can briefly leave exited Chromium children as zombies. Shared process inspection now ignores zombies because they cannot own or mutate the profile, while still failing closed on inspection errors and treating every live Chrome/Chromium process as an owner. Focused tests cover this recovery boundary.
- Review-patch candidate readiness used the page CDP target and an isolated `OffscreenCanvas` to create a real WebGL2 context and require `EXT_color_buffer_float` plus `OES_texture_float_linear` without touching Onshape's DOM canvases. The complete health call shares one two-second deadline across HTTP, browser-CDP, and page-CDP stages; supervisor startup is capped at 35 seconds.
- A second hardened GPU fault injection proved the debounce: after GPU PID 128 was killed, the first failed health interval kept main browser PID 64 alive; the second consecutive failure triggered graceful recovery to PID 839. The restored browser passed the page-level WebGL2 probe with zero GPU crashes, and the persistent home/workspace markers remained intact.
- The full `docker build` did not complete locally because the Mac/OrbStack filesystem had only 1.9 GiB free and package extraction hit `No space left on device`. No user Docker data was pruned. A reproducible full image build plus the real Onshape Grill Module interaction therefore remain release checks.

## Suggested Review Order

**Renderer policy**

- Centralize one Mesa/ANGLE policy and normalize every caller's effective profile.
  [`sprig-desktop-browser.sh:30`](../../scripts/sprig-desktop-browser.sh#L30)

- Package the Mesa runtime and health executable into every desktop PC image.
  [`Dockerfile.sprig-desktop:96`](../../Dockerfile.sprig-desktop#L96)

**WebGL health and recovery**

- Prove browser GPU state and an isolated page-level WebGL2 context through CDP.
  [`sprig-desktop-webgl-health.mjs:224`](../../scripts/sprig-desktop-webgl-health.mjs#L224)

- Require Onshape's float extensions without touching the application's DOM canvases.
  [`sprig-desktop-webgl-health.mjs:98`](../../scripts/sprig-desktop-webgl-health.mjs#L98)

- Gate startup readiness and debounce runtime recovery across consecutive failures.
  [`sprig-desktop-supervise.sh:296`](../../scripts/sprig-desktop-supervise.sh#L296)

- Ignore exited zombies while failing closed on process-inspection errors.
  [`sprig-desktop-profile-locks.sh:10`](../../scripts/sprig-desktop-profile-locks.sh#L10)

**Regression proof**

- Exercise conflicting flags, duplicate profiles, packaging, and supervisor wiring.
  [`test-sprig-desktop-webgl-config.sh:31`](../../scripts/test-sprig-desktop-webgl-config.sh#L31)

- Cover GPU, WebGL2, extensions, HTTP, websocket, timeout, and CDP failure states.
  [`test-sprig-desktop-webgl-health.mjs:65`](../../scripts/test-sprig-desktop-webgl-health.mjs#L65)

- Keep focused WebGL regressions in the repository's normal unit gate.
  [`Justfile:315`](../../Justfile#L315)
