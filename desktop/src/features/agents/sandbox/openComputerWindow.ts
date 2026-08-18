import { invokeTauri } from "@/shared/api/tauri";

/** Sandbox info the caller already has, handed to the pop-out window so it
 *  can render immediately instead of waiting for its own community/relay
 *  bootstrap to repopulate the shared sandbox store. Every field is
 *  optional — `ComputerWindowScreen` falls back to the store for whatever
 *  is missing. */
export type ComputerWindowParams = {
  viewerUrl?: string | null;
  sandboxName?: string | null;
  ownerPubkey?: string | null;
  agentDisplayName?: string | null;
  expiresAt?: number | null;
};

/**
 * Detach a sandbox's live view into its own native OS window, movable to a
 * second monitor. Mirrors `openHuddleWindow` — the backend dedups by label
 * (`computer-<sandboxId>`), so a second call while the window is already
 * open just shows/focuses it instead of opening a duplicate.
 *
 * `params` carries the caller's already-known sandbox info (viewer URL,
 * name, owner) through to the new window's URL — see `ComputerWindowParams`.
 */
export async function openComputerWindow(
  sandboxId: string,
  title: string,
  params?: ComputerWindowParams,
): Promise<void> {
  await invokeTauri<void>("open_computer_window", {
    sandboxId,
    title,
    params: params ?? null,
  });
}
