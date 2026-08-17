import * as React from "react";
import { FolderInput, GraduationCap, Monitor, X } from "lucide-react";
import { toast } from "sonner";

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/shared/ui/dialog";
import { Button } from "@/shared/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { SandboxControlToggle } from "./SandboxControlToggle";
import { SandboxDock } from "./SandboxDock";
import { SandboxFilesView } from "./SandboxFilesView";
import { type LaunchableApp, launchSandboxApp } from "./sandboxLaunch";
import { actOnSandboxWindow, type SandboxWindow } from "./sandboxWindows";
import { useSandboxWindows } from "./useSandboxWindows";
import { useContainedAspectBox } from "./useContainedAspectBox";

/** The sandbox screen is a fixed 1920x1080 remote desktop. */
const SCREEN_ASPECT_RATIO = 16 / 9;

/**
 * The agent's workspace: one live screen, the real-desktop metaphor all the
 * way through. There is no flat Files/Terminal panel to switch to anymore —
 * the dock's Browser/Files/Terminal icons open real windows on the
 * sandbox's own desktop (see `sandboxLaunch.ts`), which appear right there
 * in the stream, draggable like any other window. Computer is just "look at
 * the desktop" — the always-visible stage itself, not a separate view.
 *
 * The one thing the in-desktop file manager can't do — move files between
 * this Mac and the sandbox — lives behind "Transfer files" in the top bar,
 * opening the old file browser as an overlay over the stage.
 *
 * Control is a pure local toggle: the transparent overlay over the stage is
 * the only thing that actually blocks pointer events, so the underlying VNC
 * session is never touched by this state. Launching an app also flips
 * control to the user, since asking for a window means they're about to use
 * it.
 */
export function SandboxViewerDialog({
  open,
  onOpenChange,
  viewerUrl,
  sandboxId,
  sandboxName,
  agentDisplayName,
  remaining,
  expired,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  viewerUrl: string;
  sandboxId: string;
  sandboxName: string | null;
  agentDisplayName: string | null;
  remaining: string | null;
  expired: boolean;
}) {
  const [userInControl, setUserInControl] = React.useState(false);
  const [launching, setLaunching] = React.useState<LaunchableApp | null>(null);
  const [transferOpen, setTransferOpen] = React.useState(false);
  // State-backed callback ref: the stage element mounts only when the
  // dialog opens, so the measuring hook must re-run when it appears.
  const [stageContainer, setStageContainer] =
    React.useState<HTMLDivElement | null>(null);
  const stageBox = useContainedAspectBox(stageContainer, SCREEN_ASPECT_RATIO);

  // A freshly reopened dialog should always start with the agent in
  // control and the transfer panel closed — this state is local to one
  // workspace session.
  React.useEffect(() => {
    if (open) {
      setUserInControl(false);
      setTransferOpen(false);
      setLaunching(null);
    }
  }, [open]);

  const { windows, refresh: refreshWindows } = useSandboxWindows(
    sandboxId,
    open && !expired,
  );

  async function handleWindowClick(w: SandboxWindow) {
    // Using the taskbar is using the computer, same as launching an app.
    setUserInControl(true);
    try {
      await actOnSandboxWindow(
        sandboxId,
        w.id,
        w.active ? "minimize" : "activate",
      );
    } catch {
      // The window may have closed between poll and click — the refresh
      // below drops it from the taskbar; nothing to tell the user.
    }
    refreshWindows();
  }

  async function handleLaunch(app: LaunchableApp) {
    if (launching) return;
    setLaunching(app);
    try {
      await launchSandboxApp(sandboxId, app);
      // A launch is a request to use the window right away.
      setUserInControl(true);
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : `Could not open ${app}.`,
      );
    } finally {
      setLaunching(null);
    }
  }

  const title = agentDisplayName
    ? `${agentDisplayName}'s computer`
    : "Agent's computer";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[95vh] w-[95vw] max-w-[1600px] flex-col gap-0 p-0">
        <DialogHeader className="flex-row items-center justify-between gap-4 border-b border-border px-5 py-3 pr-12">
          <div className="min-w-0">
            <DialogTitle
              title={sandboxName ?? undefined}
              className="flex items-center gap-2 text-base"
            >
              <Monitor className="h-4 w-4 shrink-0" />
              <span className="truncate">{title}</span>
            </DialogTitle>
            <DialogDescription className="mt-0.5 text-2xs">
              {remaining && !expired ? remaining : null}
            </DialogDescription>
          </div>

          <div className="flex shrink-0 items-center gap-3">
            {!expired ? (
              <SandboxControlToggle
                userInControl={userInControl}
                onToggle={() => setUserInControl((v) => !v)}
              />
            ) : null}

            {!expired ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                data-testid="sandbox-transfer-files"
                onClick={() => setTransferOpen(true)}
                className="gap-1.5 text-2xs"
              >
                <FolderInput className="h-3.5 w-3.5" />
                Transfer files
              </Button>
            ) : null}

            <Tooltip>
              <TooltipTrigger asChild>
                <span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled
                    data-testid="sandbox-teach-task"
                    className="gap-1.5 text-2xs"
                  >
                    <GraduationCap className="h-3.5 w-3.5" />
                    Teach a task
                  </Button>
                </span>
              </TooltipTrigger>
              <TooltipContent>Coming soon</TooltipContent>
            </Tooltip>
          </div>
        </DialogHeader>

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
                The sandbox reached its expiry and was destroyed. Its screen is
                no longer available.
              </p>
            </div>
          ) : (
            // The stage is a strict 16:9 box — the remote screen is
            // 1920x1080 — measured to fit the available area (minus the top
            // bar and dock) so the iframe maps edge-to-edge with no visible
            // dead space inside it. Any leftover margin outside the box is
            // the app's surface background, not black. One screen, always
            // visible: launching an app opens a window right there in the
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
                  onClick={() => setUserInControl(true)}
                  aria-label="Take over control"
                  className="absolute inset-0 cursor-pointer bg-transparent"
                />
              ) : null}

              {/* The dock overlays the bottom edge of the screen itself, so
                  it reads as the computer's own dock sitting on the desktop
                  — not a strip of external controls floating under a VNC
                  window. It renders after the take-over overlay, so it stays
                  clickable in both control states. */}
              <div className="pointer-events-none absolute inset-x-0 bottom-2 z-10 flex justify-center [&>*]:pointer-events-auto">
                <SandboxDock
                  launching={launching}
                  onLaunch={handleLaunch}
                  windows={windows}
                  onWindowClick={handleWindowClick}
                />
              </div>

              {/* "Transfer files" overlay: moving files between this Mac and
                  the sandbox is the one thing the in-desktop file manager
                  can't do, so it lives here instead of on the dock. */}
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
      </DialogContent>
    </Dialog>
  );
}
