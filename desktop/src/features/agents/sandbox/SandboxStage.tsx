import * as React from "react";
import { Monitor, X } from "lucide-react";

import { SandboxFilesView } from "./SandboxFilesView";
import { sandboxHeartbeat, shouldSendHeartbeat } from "./sandboxHeartbeat";
import { useContainedAspectBox } from "./useContainedAspectBox";

/** The sandbox screen is a fixed 1920x1080 remote desktop. */
const SCREEN_ASPECT_RATIO = 16 / 9;

/** How often a mounted, visible stage pings the broker to keep its computer
 *  alive — well under the broker's keepalive grace window so a brief network
 *  blip between pings never drops the box (see the broker's `/heartbeat`). */
const HEARTBEAT_INTERVAL_MS = 120_000;

export type SandboxStageHandle = {
  /** Open the "Transfer files" overlay — driven by a host surface's own
   *  header button, since the header lives outside the stage. */
  openTransfer: () => void;
};

/**
 * The live view of an agent's computer: one screen (iframe), the
 * control-toggle overlay, and the "Transfer files" overlay, aspect-fit to
 * whatever container it's placed in. The dock (launcher + taskbar) is no
 * longer drawn here — it runs natively inside the streamed desktop itself
 * (tint2, see Dockerfile.sprig-desktop), so it scales with the stream's
 * real resolution instead of being an app-level overlay sized for one
 * container.
 *
 * Extracted verbatim from `SandboxViewerDialog` (no behavior change) so the
 * fullscreen dialog, the sidebar preview panel, and the pop-out native
 * window all render the exact same live view rather than three copies
 * drifting apart. Control state (`userInControl`) and the transfer overlay
 * are local to the stage, same as before extraction — each mounted stage
 * (dialog, sidebar, pop-out) gets its own independent control state, which
 * is correct: taking over control in the sidebar preview shouldn't also
 * flip the pop-out window's overlay.
 */
export const SandboxStage = React.forwardRef<
  SandboxStageHandle,
  {
    viewerUrl: string;
    sandboxId: string;
    sandboxName: string | null;
    agentDisplayName: string | null;
    remaining: string | null;
    expired: boolean;
    /** Whether this surface is currently visible/mounted-live. Defaults
     *  true. */
    active?: boolean;
    userInControl: boolean;
    onUserInControlChange: (userInControl: boolean) => void;
  }
>(function SandboxStage(
  {
    viewerUrl,
    sandboxId,
    agentDisplayName,
    expired,
    active = true,
    userInControl,
    onUserInControlChange,
  },
  ref,
) {
  const [transferOpen, setTransferOpen] = React.useState(false);
  // State-backed callback ref: the stage element mounts only when the host
  // surface appears, so the measuring hook must re-run when it appears.
  const [stageContainer, setStageContainer] =
    React.useState<HTMLDivElement | null>(null);
  const stageBox = useContainedAspectBox(stageContainer, SCREEN_ASPECT_RATIO);

  React.useImperativeHandle(
    ref,
    () => ({
      openTransfer: () => setTransferOpen(true),
    }),
    [],
  );

  // Passive-watching keepalive: while this stage is mounted, visible, and
  // showing a live screen, ping the broker on an interval so a human who's
  // just watching (no clicks) keeps the computer alive the same way real
  // activity does. Best-effort — a failed ping (e.g. the box already expired)
  // must not toast or crash the view; the normal `expired` prop takes over
  // once the next kind:48200/48201 event lands.
  React.useEffect(() => {
    if (!active || expired) return;
    const tick = () => {
      if (!shouldSendHeartbeat(active, expired, document.visibilityState)) {
        return;
      }
      sandboxHeartbeat(sandboxId).catch((error: unknown) => {
        console.warn(
          `[SandboxStage] heartbeat failed for ${sandboxId}:`,
          error,
        );
      });
    };
    const interval = window.setInterval(tick, HEARTBEAT_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, [sandboxId, active, expired]);

  const title = agentDisplayName
    ? `${agentDisplayName}'s computer`
    : "Agent's computer";

  return (
    <div
      ref={setStageContainer}
      className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-background p-3"
    >
      {expired ? (
        <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
          <Monitor className="h-8 w-8 text-muted-foreground" />
          <p className="text-sm font-medium text-foreground">
            This computer has been shut down
          </p>
          <p className="max-w-sm text-2xs text-muted-foreground">
            The sandbox reached its expiry and was destroyed. Its screen is no
            longer available.
          </p>
        </div>
      ) : (
        // The stage is a strict 16:9 box — the remote screen is 1920x1080 —
        // measured to fit the available area so the iframe maps edge-to-edge
        // with no visible dead space inside it. Any leftover margin outside
        // the box is the app's surface background, not black. One screen,
        // always visible: launching an app opens a window right there in the
        // stream, not a separate flat panel.
        <div
          className="relative overflow-hidden rounded-lg bg-black shadow-sm"
          style={
            stageBox
              ? { width: stageBox.width, height: stageBox.height }
              : { width: "100%", height: "100%", visibility: "hidden" }
          }
        >
          <iframe
            key={viewerUrl}
            src={viewerUrl}
            title={title}
            className="h-full w-full border-0"
            sandbox="allow-scripts allow-same-origin allow-forms"
            allow="clipboard-read; clipboard-write"
          />
          {!userInControl ? (
            <button
              type="button"
              data-testid="sandbox-control-overlay"
              onClick={() => onUserInControlChange(true)}
              aria-label="Take over control"
              className="absolute inset-0 cursor-pointer bg-transparent"
            />
          ) : null}

          {/* "Transfer files" overlay: moving files between this Mac and the
              sandbox is the one thing the in-desktop file manager can't do,
              so it lives here instead of on the dock (the dock itself now
              runs natively inside the streamed desktop — see the module
              doc comment above). */}
          {transferOpen ? (
            <div className="absolute inset-0 flex flex-col bg-background">
              <div className="flex items-center justify-between border-b border-border px-4 py-2.5">
                <span className="text-sm font-medium text-foreground">
                  Transfer files
                </span>
                <button
                  type="button"
                  data-testid="sandbox-transfer-files-close"
                  aria-label="Close transfer files"
                  onClick={() => setTransferOpen(false)}
                  className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
              <div className="min-h-0 flex-1">
                <SandboxFilesView sandboxId={sandboxId} />
              </div>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
});
