import * as React from "react";

import { listSandboxWindows, type SandboxWindow } from "./sandboxWindows";

const POLL_MS = 2000;

/**
 * The dock's live window list: polls the broker for the desktop's open
 * windows while `enabled` (the viewer dialog is open). Poll failures keep
 * the last good list — a taskbar that flickers empty on one dropped
 * request reads as windows closing. `refresh` forces an immediate re-poll
 * (after a click action, so the highlight moves without waiting a tick).
 */
export function useSandboxWindows(
  sandboxId: string,
  enabled: boolean,
): { windows: SandboxWindow[]; refresh: () => void } {
  const [windows, setWindows] = React.useState<SandboxWindow[]>([]);
  const [nonce, setNonce] = React.useState(0);
  const inFlight = React.useRef(false);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `nonce` is the refresh trigger — bumping it forces an immediate re-poll after a taskbar action.
  React.useEffect(() => {
    if (!enabled) {
      setWindows([]);
      return;
    }
    let cancelled = false;
    async function tick() {
      if (inFlight.current) return;
      inFlight.current = true;
      try {
        const next = await listSandboxWindows(sandboxId);
        if (!cancelled) setWindows(next);
      } catch {
        // Keep the previous list; the next tick retries.
      } finally {
        inFlight.current = false;
      }
    }
    void tick();
    const interval = window.setInterval(() => void tick(), POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [sandboxId, enabled, nonce]);

  const refresh = React.useCallback(() => setNonce((n) => n + 1), []);
  return { windows, refresh };
}
