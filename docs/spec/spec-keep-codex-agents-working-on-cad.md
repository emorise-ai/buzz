---
title: 'Keep Codex agents working on CAD tasks'
type: 'bugfix'
created: '2026-08-31'
status: 'done'
review_loop_iteration: 0
baseline_commit: 9cced4a5d4ba1c329203458143ea33599537837c
context:
  - VISION_AGENT.md
  - TESTING.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Codex-backed Buzz agents can end a turn after publishing a future-tense progress message even though delegated work remains unfinished. Onshape also drives Chromium past the current 4 GiB self-service PC limit, causing repeated cgroup OOM kills and browser restarts.

**Approach:** Require Codex agents to use their native persistent goal for explicitly delegated work that needs multiple turns, keeping it active until a verified result or concrete blocker. Give self-service browser PCs 8 GiB explicitly while retaining the 4 GiB default for ordinary non-CAD agent containers.

## Boundaries & Constraints

**Always:** Preserve user steering and cancellation; do not treat status text as completion; use the runtime's native goal lifecycle when available; keep the computer's persistent home and workspace volumes; apply the 8 GiB request to both desktop-started and agent-started PCs; verify the real Emily and Onshape workflow.

**Ask First:** Any change that makes all broker-managed or remote-agent containers default to 8 GiB; any new autonomous scheduler outside the existing Codex goal lifecycle; any reduction in sandbox concurrency or host-wide resource limits.

**Never:** Infer a persistent goal for a simple question or one-turn answer; keep a goal active after its deliverable is verified or it is genuinely blocked; hide OOM or browser failure behind progress messages; delete or replace Emily's persistent volumes.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|----------------------------|----------------|
| Multi-turn assignment | Human delegates work that cannot finish in one turn and Codex goal tools are available | Agent creates a concrete goal before any pickup/status post and continues across turn boundaries until completion or blocker | Goal remains active through status updates; cancellation and steering still work |
| Simple answer | Human asks a question answerable in the current turn | Agent answers normally without creating a persistent goal | Normal end-turn behavior remains unchanged |
| Desktop PC creation | User starts an agent computer from Buzz Desktop | Broker receives `memory_mb: 8192` | Broker retains its existing 8 GiB clamp |
| Agent PC creation | Agent runs `buzz sandbox create` without a memory option | CLI sends `memory_mb: 8192` | Explicit `--memory-mb` still overrides and broker clamps it |
| Existing 4 GiB PC | Owner already has a running sandbox | New setting takes effect only after destroy/recreate; named volumes survive | Never delete persistent volumes during recreation |

</frozen-after-approval>

## Code Map

- `crates/buzz-acp/src/base_prompt.md` -- standing instructions shared with managed agents; add the native-goal persistence contract.
- `crates/buzz-acp/src/lib.rs` -- prompt contract tests.
- `desktop/src-tauri/src/sandbox_viewer.rs` -- desktop self-service PC create request.
- `desktop/src-tauri/src/sandbox_viewer_tests.rs` -- desktop request-body regression coverage.
- `crates/buzz-cli/src/lib.rs` -- `buzz sandbox create` memory default and argument tests.
- `crates/buzz-cli/src/commands/sandbox.rs` -- agent-created PC request body.
- `crates/buzz-sandbox-broker/src/sandbox.rs` -- existing 8 GiB ceiling and 4 GiB general default, unchanged but covered during verification.

## Tasks & Acceptance

**Execution:**
- [x] `crates/buzz-acp/src/base_prompt.md`, `crates/buzz-acp/src/lib.rs` -- require native persistent goals for explicit multi-turn assignments and lock the wording with tests.
- [x] `desktop/src-tauri/src/sandbox_viewer.rs`, `desktop/src-tauri/src/sandbox_viewer_tests.rs` -- send and test an explicit 8192 MiB limit for desktop-created PCs.
- [x] `crates/buzz-cli/src/lib.rs`, `crates/buzz-cli/src/commands/sandbox.rs` -- default agent-created PCs to 8192 MiB while preserving explicit overrides.
- [x] Build the desktop and desktop-PC image, deploy the needed artifacts, recreate Emily's disposable container without touching its named volumes, and exercise the live workflow.

**Acceptance Criteria:**
- Given a delegated multi-turn task, when Emily publishes an interim update, then her Codex goal remains active and another work turn starts without Ron prompting her again.
- Given a new self-service PC, when it starts through either supported creation path, then Docker reports an 8 GiB memory limit.
- Given Emily's persisted Onshape profile and document, when the real CAD workflow runs, then WebGL2 remains usable and Matterhorn records no new Chromium cgroup OOM kill during the observation window.
- Given an ordinary remote agent container, when it uses the broker or Docker-provider default, then its existing 4 GiB default remains unchanged.

## Spec Change Log

- 2026-08-31: Implemented local coverage for the native-goal prompt contract and both 8192 MiB self-service PC creation paths. Restricted the CLI default to the desktop PC image so other images retain the broker's 4096 MiB default. Live deployment and workflow acceptance remain pending.
- 2026-08-31: Installed the rebuilt desktop app, deployed `buzz-sprig-desktop:latest` to Matterhorn, and recreated Emily's PC with its original named volumes. Docker reported an 8 GiB limit and zero OOM kills. The persisted Onshape login and TLX Grill Module reopened; direct CDP probes reported WebGL and WebGL2 available with no context-loss message after more than 10 minutes. A fresh delegated two-stage probe created a native goal, published an interim update, continued without another human prompt, published the final comparison, and marked the goal complete.

## Design Notes

Codex ACP already exposes the runtime's goal controls and automatic continuation behavior. Buzz should tell Codex when to use that mechanism instead of implementing a parallel retry loop that would have to duplicate goal completion, pause, resume, cancellation, and steering semantics.

The broker already allows at most 8192 MiB. The desktop and self-service CLI are the CAD-computer entry points, so they request the ceiling explicitly. The general broker and Docker-provider defaults stay at 4096 MiB to avoid doubling memory for non-browser agent bodies.

## Verification

**Commands:**
- `cargo test -p buzz-acp` -- prompt contract and ACP harness tests pass.
- `cargo test -p buzz-cli` -- sandbox-create default and override tests pass.
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml sandbox_viewer` -- desktop request-body tests pass.
- `cargo test -p buzz-sandbox-broker` -- broker limits and no-swap policy remain green.
- `cargo test -p buzz-backend-docker` -- the ordinary remote-agent 4096 MiB default remains green.
- `just ci` -- repository-wide checks pass before commit.

**Manual checks:**
- Passed: Emily's recreated PC reported Docker `HostConfig.Memory=8589934592` and retained the same named `/home/agent` and `/workspace` volumes.
- Passed: the persisted Onshape session reopened `TLX Modules | Grill Module`; direct browser probes reported WebGL and WebGL2 available, one live model canvas, and no context-loss message after the CAD-active observation window. `memory.events` remained at `oom=0` and `oom_kill=0`.
- Passed: Emily called native `create_goal`, published an interim update, continued to the scheduled second check without another human prompt, published the final comparison, and called `update_goal({status:"complete"})`.
- Passed: the rebuilt app was installed at `/Applications/Buzz.app`; Matterhorn's deployed desktop-PC image is `sha256:56a968d4a0808d2615f28ef6a67822ad58e697fa3105b1e75d6957f62006c9a0`. No relay source changed, so no relay deployment was required.

## Suggested Review Order

1. Agent persistence contract: [`base_prompt.md`](../../crates/buzz-acp/src/base_prompt.md) and its regression test in [`lib.rs`](../../crates/buzz-acp/src/lib.rs).
2. Agent-created PC resource request: [`sandbox.rs`](../../crates/buzz-cli/src/commands/sandbox.rs) and CLI argument retention in [`lib.rs`](../../crates/buzz-cli/src/lib.rs).
3. Desktop-created PC resource request: [`sandbox_viewer.rs`](../../desktop/src-tauri/src/sandbox_viewer.rs) and [`sandbox_viewer_tests.rs`](../../desktop/src-tauri/src/sandbox_viewer_tests.rs).
4. Verification evidence and deployment record in this spec.
