import * as React from "react";
import { ExternalLink, Maximize2, Monitor } from "lucide-react";
import { toast } from "sonner";

import {
  AuxiliaryPanel,
  AuxiliaryPanelBody,
  AuxiliaryPanelHeader,
  AuxiliaryPanelHeaderActions,
  AuxiliaryPanelHeaderGroup,
} from "@/shared/layout/AuxiliaryPanel";
import type { AuxiliaryPanelLayout } from "@/shared/layout/AuxiliaryPanel";
import { Button } from "@/shared/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { cn } from "@/shared/lib/cn";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { resolveProfileDisplayName } from "@/features/profile/ui/UserProfilePanelUtils";
import { useNow } from "@/shared/lib/useNow";
import { useAgentSandbox } from "./useAgentSandbox";
import { useHasEverHadComputer } from "./computerEverAssignedStore";
import { formatSandboxRemaining, isSandboxExpired } from "./sandboxCountdown";
import { mintViewerUrl } from "./mintViewerUrl";
import { openComputerWindow } from "./openComputerWindow";
import { SandboxStage } from "./SandboxStage";
import { SandboxViewerDialog } from "./SandboxViewerDialog";
import { consumeComputerViewerRequest } from "./computerPanelStore";

/**
 * Live sidebar view of an agent's computer, opened from the Computer button
 * in the channel header. Same live view as the fullscreen dialog and the
 * pop-out window (`SandboxStage`), just docked in the right auxiliary pane
 * instead of a modal — watch the desktop while still chatting.
 */
export function ComputerPreviewPanel({
  ownerPubkey,
  isSinglePanelView = false,
  layout = "standalone",
  transparentChrome = false,
  openViewerOnReady = false,
  widthPx,
  onClose,
}: {
  ownerPubkey: string;
  isSinglePanelView?: boolean;
  layout?: AuxiliaryPanelLayout;
  transparentChrome?: boolean;
  openViewerOnReady?: boolean;
  widthPx: number;
  onClose: () => void;
}) {
  const sandbox = useAgentSandbox(ownerPubkey);
  const everHadComputer = useHasEverHadComputer(ownerPubkey);
  const now = useNow(1000);
  const { data: profile } = useUserProfileQuery(ownerPubkey);
  const agentDisplayName = resolveProfileDisplayName({
    persona: undefined,
    profile,
    pubkey: ownerPubkey,
  });
  const [userInControl, setUserInControl] = React.useState(false);
  const [mintedUrl, setMintedUrl] = React.useState<string | null>(null);
  const [minting, setMinting] = React.useState(false);
  const [viewerOpen, setViewerOpen] = React.useState(false);

  const viewerUrl = sandbox?.viewerUrl ?? null;

  // Mint a fresh signed link whenever the panel opens for a viewer URL, or
  // the viewer URL itself changes (e.g. a new sandbox for the same owner) —
  // the token is only valid briefly, so it isn't minted ahead of time.
  React.useEffect(() => {
    setMintedUrl(null);
    if (!viewerUrl) return;
    let cancelled = false;
    setMinting(true);
    mintViewerUrl(viewerUrl)
      .then((url) => {
        if (!cancelled) setMintedUrl(url);
      })
      .catch((err) => {
        console.error("[ComputerPreviewPanel] mint failed:", err);
        if (!cancelled) {
          toast.error("Could not open the agent's screen. Try again.");
        }
      })
      .finally(() => {
        if (!cancelled) setMinting(false);
      });
    return () => {
      cancelled = true;
    };
  }, [viewerUrl]);

  const title = `${agentDisplayName}'s computer`;
  const expired =
    sandbox?.expiresAt != null && isSandboxExpired(sandbox.expiresAt, now);
  const remaining =
    sandbox?.expiresAt != null
      ? formatSandboxRemaining(sandbox.expiresAt, now)
      : null;

  React.useEffect(() => {
    if (!openViewerOnReady || !sandbox || !mintedUrl || expired) return;
    setViewerOpen(true);
    consumeComputerViewerRequest(ownerPubkey);
  }, [expired, mintedUrl, openViewerOnReady, ownerPubkey, sandbox]);

  async function handlePopOut() {
    if (!sandbox) return;
    try {
      await openComputerWindow(sandbox.id, title, {
        viewerUrl: sandbox.viewerUrl,
        sandboxName: sandbox.name,
        ownerPubkey,
        agentDisplayName,
        expiresAt: sandbox.expiresAt,
      });
    } catch (err) {
      console.error("[ComputerPreviewPanel] pop-out failed:", err);
      toast.error(
        err instanceof Error ? err.message : "Could not open a new window.",
      );
    }
  }

  return (
    <>
      <AuxiliaryPanel
        isSinglePanelView={isSinglePanelView}
        layout={layout}
        onClose={onClose}
        testId="computer-preview-panel"
        transparentChrome={transparentChrome}
        widthPx={widthPx}
        header={
          <AuxiliaryPanelHeader
            backdrop={layout !== "split"}
            backdropSurface="soft"
            inset={layout !== "split" ? "wide" : "default"}
          >
            <AuxiliaryPanelHeaderGroup
              align="start"
              leading={<Monitor className="h-4 w-4 shrink-0" />}
            >
              <div className="min-w-0 flex-1">
                <h2
                  className="truncate text-sm font-semibold leading-5"
                  data-testid="computer-preview-panel-title"
                  title={title}
                >
                  {title}
                </h2>
                {remaining && !expired ? (
                  <p className="truncate text-2xs text-muted-foreground">
                    {remaining}
                  </p>
                ) : null}
              </div>
            </AuxiliaryPanelHeaderGroup>
            <AuxiliaryPanelHeaderActions includeCloseAction>
              {sandbox ? (
                <>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        data-testid="computer-preview-full-screen"
                        aria-label="Open full screen"
                        disabled={!mintedUrl || expired}
                        onClick={() => setViewerOpen(true)}
                      >
                        <Maximize2 className="h-4 w-4" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>Open full screen</TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        data-testid="computer-preview-pop-out"
                        aria-label="Open in a new window"
                        onClick={() => void handlePopOut()}
                      >
                        <ExternalLink className="h-4 w-4" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>Open in a new window</TooltipContent>
                  </Tooltip>
                </>
              ) : null}
            </AuxiliaryPanelHeaderActions>
          </AuxiliaryPanelHeader>
        }
      >
        <AuxiliaryPanelBody className="flex min-h-0 flex-1 flex-col overflow-hidden p-0">
          {!sandbox ? (
            <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
              <Monitor
                className={cn(
                  "h-8 w-8 text-muted-foreground",
                  everHadComputer && "animate-pulse",
                )}
              />
              <p className="text-sm font-medium text-foreground">
                {everHadComputer ? "Starting computer…" : "No computer running"}
              </p>
            </div>
          ) : mintedUrl ? (
            <SandboxStage
              viewerUrl={mintedUrl}
              sandboxId={sandbox.id}
              sandboxName={sandbox.name}
              agentDisplayName={agentDisplayName}
              remaining={remaining}
              expired={expired}
              userInControl={userInControl}
              onUserInControlChange={setUserInControl}
            />
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
              <Monitor
                className={
                  minting
                    ? "h-8 w-8 animate-pulse text-muted-foreground"
                    : "h-8 w-8 text-muted-foreground"
                }
              />
              <p className="text-2xs text-muted-foreground">
                {minting ? "Connecting…" : "Screen not reachable"}
              </p>
            </div>
          )}
        </AuxiliaryPanelBody>
      </AuxiliaryPanel>
      {sandbox && mintedUrl ? (
        <SandboxViewerDialog
          agentDisplayName={agentDisplayName}
          expired={expired}
          expiresAt={sandbox.expiresAt}
          onOpenChange={setViewerOpen}
          open={viewerOpen}
          ownerPubkey={ownerPubkey}
          rawViewerUrl={sandbox.viewerUrl ?? undefined}
          remaining={remaining}
          sandboxId={sandbox.id}
          sandboxName={sandbox.name}
          viewerUrl={mintedUrl}
        />
      ) : null}
    </>
  );
}
