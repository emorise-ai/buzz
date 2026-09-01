import * as React from "react";
import { Loader2, Monitor, RefreshCw, X } from "lucide-react";

import {
  copyTextToSystemClipboard,
  readTextFromSystemClipboard,
} from "@/shared/api/tauriMedia";

import { SandboxFilesView } from "./SandboxFilesView";
import { sandboxHeartbeat, shouldSendHeartbeat } from "./sandboxHeartbeat";
import { mintViewerUrl } from "./mintViewerUrl";
import { useContainedAspectBox } from "./useContainedAspectBox";

/** The sandbox screen is a fixed 1920x1080 remote desktop. */
const SCREEN_ASPECT_RATIO = 16 / 9;

/** How often a mounted, visible stage pings the broker to keep its computer
 *  alive — well under the broker's keepalive grace window so a brief network
 *  blip between pings never drops the box (see the broker's `/heartbeat`). */
const HEARTBEAT_INTERVAL_MS = 120_000;
/** A viewer must either prove its RFB connection or be reminted. This bounds
 * every loading state instead of leaving an opaque overlay up forever. */
const VIEWER_CONNECT_TIMEOUT_MS = 10_000;
const MAX_AUTOMATIC_VIEWER_REFRESHES = 2;

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
    /** Bare broker viewer URL used to renew a short-lived signed viewer URL
     * after noVNC disconnects. Data/mock viewers may omit it. */
    rawViewerUrl?: string;
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
    rawViewerUrl,
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
  const iframeRef = React.useRef<HTMLIFrameElement | null>(null);
  const refreshableViewerUrl =
    rawViewerUrl?.startsWith("https://") || rawViewerUrl?.startsWith("http://")
      ? rawViewerUrl
      : null;
  const [frameUrl, setFrameUrl] = React.useState(viewerUrl);
  const [connectionState, setConnectionState] = React.useState<
    "connecting" | "connected" | "reconnecting" | "failed"
  >("connecting");
  const refreshInFlight = React.useRef(false);
  const refreshAttempts = React.useRef(0);

  React.useEffect(() => {
    setFrameUrl(viewerUrl);
    setConnectionState(refreshableViewerUrl ? "connecting" : "connected");
    refreshInFlight.current = false;
    refreshAttempts.current = 0;
  }, [refreshableViewerUrl, viewerUrl]);

  const refreshViewer = React.useCallback(
    async (resetAttempts = false) => {
      if (!refreshableViewerUrl || refreshInFlight.current) return;
      if (resetAttempts) refreshAttempts.current = 0;
      if (refreshAttempts.current >= MAX_AUTOMATIC_VIEWER_REFRESHES) {
        setConnectionState("failed");
        return;
      }
      refreshAttempts.current += 1;
      refreshInFlight.current = true;
      setConnectionState("reconnecting");
      try {
        setFrameUrl(await mintViewerUrl(refreshableViewerUrl));
      } catch {
        setConnectionState("failed");
      } finally {
        refreshInFlight.current = false;
      }
    },
    [refreshableViewerUrl],
  );

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

  // Clipboard bridge between this Mac and the sandbox's desktop.
  //
  // The viewer is a cross-origin, sandboxed iframe, so the DOM Clipboard API
  // inside it is refused outright (`NotAllowedError`) regardless of user
  // gesture — a sandboxed frame gets an opaque origin and `clipboard-read` is
  // not grantable there. The app shell, by contrast, is a Tauri webview with
  // a native clipboard path (`read_clipboard_text` / `copy_text_to_clipboard`,
  // arboard-backed). So the shell does the clipboard I/O and exchanges plain
  // text with the viewer over postMessage:
  //   viewer -> "buzz:clipboard-request"      (user is about to interact)
  //   shell  -> "buzz:clipboard" {text}       (current host clipboard)
  //   viewer -> "buzz:clipboard-copy" {text}  (VM's selection changed)
  React.useEffect(() => {
    if (expired) return;
    const onMessage = (event: MessageEvent) => {
      const frame = iframeRef.current;
      // Only listen to our own viewer frame.
      if (!frame || event.source !== frame.contentWindow) return;
      if (
        typeof event.data !== "object" ||
        event.data === null ||
        typeof Reflect.get(event.data, "type") !== "string"
      ) {
        return;
      }
      const type = Reflect.get(event.data, "type");

      if (type === "buzz:clipboard-request") {
        void readTextFromSystemClipboard()
          .then((text) => {
            if (!text) return;
            frame.contentWindow?.postMessage(
              { type: "buzz:clipboard", text },
              "*",
            );
          })
          .catch(() => {
            // Nothing on the clipboard, or the read failed — paste simply
            // has nothing to deliver this time.
          });
        return;
      }

      if (type === "buzz:viewer-connected") {
        refreshAttempts.current = 0;
        setConnectionState("connected");
        return;
      }

      if (type === "buzz:viewer-disconnected") {
        setConnectionState("reconnecting");
        return;
      }

      if (type === "buzz:viewer-refresh-request") {
        void refreshViewer();
        return;
      }

      if (
        type === "buzz:clipboard-copy" &&
        typeof Reflect.get(event.data, "text") === "string"
      ) {
        void copyTextToSystemClipboard(Reflect.get(event.data, "text")).catch(
          () => {
            // Copying out of the VM is best-effort; a failure here must not
            // disturb the live view.
          },
        );
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [expired, refreshViewer]);

  // The viewer can establish RFB before React's message effect is installed.
  // Ask for its current state after the iframe loads so connection reporting is
  // a handshake, not a race-prone one-shot notification.
  const requestViewerStatus = React.useCallback(() => {
    iframeRef.current?.contentWindow?.postMessage(
      { type: "buzz:viewer-status-request" },
      "*",
    );
  }, []);

  // A broken viewer, proxy, or message bridge must reach a useful failure UI.
  // Each successful remint changes frameUrl and starts one fresh bounded wait.
  React.useEffect(() => {
    if (
      !refreshableViewerUrl ||
      connectionState === "connected" ||
      connectionState === "failed"
    ) {
      return;
    }
    const pendingFrameUrl = frameUrl;
    const timeout = window.setTimeout(() => {
      if (iframeRef.current?.src === pendingFrameUrl) void refreshViewer();
    }, VIEWER_CONNECT_TIMEOUT_MS);
    return () => window.clearTimeout(timeout);
  }, [connectionState, frameUrl, refreshViewer, refreshableViewerUrl]);

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
            key={frameUrl}
            ref={iframeRef}
            src={frameUrl}
            title={title}
            className="h-full w-full border-0"
            sandbox="allow-scripts allow-same-origin allow-forms"
            allow="clipboard-read; clipboard-write"
            onLoad={requestViewerStatus}
          />
          {connectionState !== "connected" && refreshableViewerUrl ? (
            <div
              className="absolute inset-0 z-20 flex flex-col items-center justify-center gap-2 bg-background/90 text-center"
              data-testid="sandbox-connection-state"
            >
              {connectionState === "failed" ? (
                <>
                  <Monitor className="h-8 w-8 text-muted-foreground" />
                  <p className="text-sm font-medium text-foreground">
                    Screen connection lost
                  </p>
                  <button
                    type="button"
                    className="flex items-center gap-1.5 rounded-md border border-border bg-background px-3 py-1.5 text-xs text-foreground hover:bg-muted"
                    onClick={() => void refreshViewer(true)}
                  >
                    <RefreshCw className="h-3.5 w-3.5" />
                    Retry
                  </button>
                </>
              ) : (
                <>
                  <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
                  <p className="text-sm font-medium text-foreground">
                    {connectionState === "connecting"
                      ? "Connecting to computer…"
                      : "Reconnecting to computer…"}
                  </p>
                </>
              )}
            </div>
          ) : null}
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
