import { invokeTauri } from "@/shared/api/tauri";

/**
 * Bump a sandbox's expiry — the viewer's passive-watching keepalive.
 * `SandboxStage` calls this on an interval while its screen is mounted and
 * visible, so a human just watching (no clicks) keeps the computer alive the
 * same way real activity does.
 *
 * Best-effort by design: a box that's already gone answers 404, which
 * surfaces as a rejected promise here. Callers must not toast or crash the
 * view on failure — the viewer's normal expired-state path takes over once
 * the next kind:48200/48201 event lands.
 */
export async function sandboxHeartbeat(sandboxId: string): Promise<void> {
  await invokeTauri("sandbox_heartbeat", { sandboxId });
}

/**
 * Pure gate for whether a stage's interval tick should actually ping: only
 * while mounted-live (`active`, not `expired`) and while the tab/window is
 * visible — a hidden background viewer (e.g. a pop-out window dragged behind
 * something, or the app minimized) must not hold a computer alive forever.
 * Split out from the `useEffect` in `SandboxStage` so this decision is
 * directly testable without mounting React or mocking Tauri.
 */
export function shouldSendHeartbeat(
  active: boolean,
  expired: boolean,
  visibilityState: DocumentVisibilityState,
): boolean {
  return active && !expired && visibilityState !== "hidden";
}
