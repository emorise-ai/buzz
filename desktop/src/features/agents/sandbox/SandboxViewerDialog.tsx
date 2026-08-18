import * as React from "react";
import {
  ExternalLink,
  FolderInput,
  GraduationCap,
  Monitor,
} from "lucide-react";
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
import { SandboxStage, type SandboxStageHandle } from "./SandboxStage";
import { openComputerWindow } from "./openComputerWindow";

/**
 * The agent's workspace: one live screen, the real-desktop metaphor all the
 * way through. There is no flat Files/Terminal panel to switch to — the
 * desktop's own native dock (tint2, running inside the sandbox — see
 * Dockerfile.sprig-desktop) opens real windows on the sandbox's own
 * desktop, which appear right there in the stream, draggable like any
 * other window. This dialog no longer draws any dock chrome of its own.
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
  rawViewerUrl,
  sandboxId,
  sandboxName,
  ownerPubkey,
  agentDisplayName,
  remaining,
  expired,
  expiresAt,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Already-minted, openable URL — what the iframe loads. */
  viewerUrl: string;
  /** The un-minted URL the relay announced, handed to the pop-out window so
   *  it can mint its own token instead of waiting on the shared sandbox
   *  store (see `openComputerWindow`). Falls back to `viewerUrl` if a
   *  caller doesn't have the raw one on hand. */
  rawViewerUrl?: string;
  sandboxId: string;
  sandboxName: string | null;
  ownerPubkey?: string | null;
  agentDisplayName: string | null;
  remaining: string | null;
  expired: boolean;
  expiresAt?: number | null;
}) {
  const [userInControl, setUserInControl] = React.useState(false);
  const stageRef = React.useRef<SandboxStageHandle>(null);

  // A freshly reopened dialog should always start with the agent in
  // control — this state is local to one workspace session.
  React.useEffect(() => {
    if (open) {
      setUserInControl(false);
    }
  }, [open]);

  const title = agentDisplayName
    ? `${agentDisplayName}'s computer`
    : "Agent's computer";

  async function handlePopOut() {
    try {
      await openComputerWindow(sandboxId, title, {
        viewerUrl: rawViewerUrl ?? viewerUrl,
        sandboxName,
        ownerPubkey,
        agentDisplayName,
        expiresAt,
      });
    } catch (err) {
      console.error("[SandboxViewerDialog] pop-out failed:", err);
      toast.error(
        err instanceof Error ? err.message : "Could not open a new window.",
      );
    }
  }

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
                onClick={() => stageRef.current?.openTransfer()}
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

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  data-testid="sandbox-pop-out"
                  aria-label="Open in a new window"
                  onClick={() => void handlePopOut()}
                >
                  <ExternalLink className="h-4 w-4" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>Open in a new window</TooltipContent>
            </Tooltip>
          </div>
        </DialogHeader>

        <SandboxStage
          ref={stageRef}
          viewerUrl={viewerUrl}
          sandboxId={sandboxId}
          sandboxName={sandboxName}
          agentDisplayName={agentDisplayName}
          remaining={remaining}
          expired={expired}
          active={open}
          userInControl={userInControl}
          onUserInControlChange={setUserInControl}
        />
      </DialogContent>
    </Dialog>
  );
}
