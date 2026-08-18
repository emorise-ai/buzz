import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

const COMPUTER_WINDOW_LABEL_PREFIX = "computer-";

/**
 * Returns the sandbox id only for a dedicated computer pop-out window
 * (opened by `open_computer_window`, labeled `computer-<sandboxId>`).
 * Mirrors `huddleWindowChannelId` — same window-label-as-route-signal
 * pattern, checked at the root route so a pop-out window renders only the
 * live view with no app chrome.
 */
export function computerWindowSandboxId(): string | null {
  if (!isTauri()) return null;

  let label: string;
  try {
    label = getCurrentWindow().label;
  } catch {
    // Browser previews can expose the Tauri IPC mock without window metadata.
    return null;
  }
  if (!label.startsWith(COMPUTER_WINDOW_LABEL_PREFIX)) return null;

  const sandboxId = label.slice(COMPUTER_WINDOW_LABEL_PREFIX.length);
  return sandboxId.length > 0 ? sandboxId : null;
}

/** Sandbox info handed off from the parent window via the pop-out's URL —
 *  see `open_computer_window`'s `ComputerWindowParams`. Every field is
 *  `null` when absent (not passed, or the parent didn't know it yet), which
 *  `ComputerWindowScreen` reads as "fall back to the shared sandbox store"
 *  for that field. */
export type ComputerWindowHandoff = {
  viewerUrl: string | null;
  sandboxName: string | null;
  ownerPubkey: string | null;
  agentDisplayName: string | null;
  expiresAt: number | null;
};

const EMPTY_HANDOFF: ComputerWindowHandoff = {
  viewerUrl: null,
  sandboxName: null,
  ownerPubkey: null,
  agentDisplayName: null,
  expiresAt: null,
};

/**
 * Read the sandbox info the parent window passed via the pop-out's own URL
 * (`#/computer/<id>?viewerUrl=...&sandboxName=...`), so the second webview
 * can render without waiting on its own community/relay bootstrap to
 * repopulate the shared sandbox store — that store starts empty in a fresh
 * window and, unlike the main window, has nothing else that would populate
 * it sooner.
 */
export function readComputerWindowHandoff(): ComputerWindowHandoff {
  const hash = window.location.hash;
  const queryStart = hash.indexOf("?");
  if (queryStart === -1) return EMPTY_HANDOFF;

  const params = new URLSearchParams(hash.slice(queryStart + 1));
  const expiresAtRaw = params.get("expiresAt");
  const expiresAt = expiresAtRaw !== null ? Number(expiresAtRaw) : null;

  return {
    viewerUrl: params.get("viewerUrl"),
    sandboxName: params.get("sandboxName"),
    ownerPubkey: params.get("ownerPubkey"),
    agentDisplayName: params.get("agentDisplayName"),
    expiresAt:
      expiresAt !== null && Number.isFinite(expiresAt) ? expiresAt : null,
  };
}
