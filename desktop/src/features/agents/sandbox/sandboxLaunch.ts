import { invokeTauri } from "@/shared/api/tauri";

/** The three app kinds the dock can open a new window of on the sandbox's
 *  own desktop. Computer has no launcher — it's just the always-visible
 *  screen, not a window to open. */
export type LaunchableApp = "browser" | "files" | "terminal";

/**
 * Open a new window of `app` on the sandbox's live desktop — the real-desktop
 * equivalent of double-clicking an app icon. The window appears in the
 * screen stream itself; this call has no return payload to react to.
 */
export async function launchSandboxApp(
  sandboxId: string,
  app: LaunchableApp,
): Promise<void> {
  await invokeTauri("sandbox_launch_app", { sandboxId, app });
}
