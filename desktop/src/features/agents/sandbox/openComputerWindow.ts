import { invokeTauri } from "@/shared/api/tauri";

/**
 * Detach a sandbox's live view into its own native OS window, movable to a
 * second monitor. Mirrors `openHuddleWindow` — the backend dedups by label
 * (`computer-<sandboxId>`), so a second call while the window is already
 * open just shows/focuses it instead of opening a duplicate.
 */
export async function openComputerWindow(
  sandboxId: string,
  title: string,
): Promise<void> {
  await invokeTauri<void>("open_computer_window", { sandboxId, title });
}
