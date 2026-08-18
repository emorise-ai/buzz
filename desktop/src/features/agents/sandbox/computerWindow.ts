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
