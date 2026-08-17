import { invokeTauri } from "@/shared/api/tauri";

/** One open window on the sandbox's desktop, as the broker reports it. */
export type SandboxWindow = {
  /** X11 window id (`0x…` hex or decimal) — opaque here, echoed back on actions. */
  id: string;
  title: string;
  /** The window's WM_CLASS instance (e.g. "chromium", "thunar",
   *  "xfce4-terminal") — what the dock maps to an app icon. Empty when the
   *  window doesn't set one. */
  wmClass: string;
  /** Whether this is the currently focused window. */
  active: boolean;
};

export type SandboxWindowAction = "activate" | "minimize" | "close";

type BrokerWindow = {
  id: string;
  title: string;
  class?: unknown;
  active: boolean;
};

function isBrokerWindow(value: unknown): value is BrokerWindow {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.id === "string" &&
    typeof v.title === "string" &&
    typeof v.active === "boolean"
  );
}

/**
 * List the open windows on the sandbox's desktop — the dock's taskbar
 * segment polls this while the viewer is open. Malformed rows from the
 * broker are dropped rather than failing the whole list.
 */
export async function listSandboxWindows(
  sandboxId: string,
): Promise<SandboxWindow[]> {
  const raw = await invokeTauri<{ windows?: unknown }>("sandbox_list_windows", {
    sandboxId,
  });
  if (!Array.isArray(raw?.windows)) return [];
  return raw.windows.filter(isBrokerWindow).map((w) => ({
    id: w.id,
    title: w.title,
    wmClass: typeof w.class === "string" ? w.class.toLowerCase() : "",
    active: w.active,
  }));
}

/**
 * Focus, minimize, or close one window on the sandbox's desktop — the
 * taskbar's click actions. A window that vanished between poll and click
 * makes the broker answer 404; callers just refresh on any failure.
 */
export async function actOnSandboxWindow(
  sandboxId: string,
  window: string,
  action: SandboxWindowAction,
): Promise<void> {
  await invokeTauri("sandbox_window_action", { sandboxId, window, action });
}
