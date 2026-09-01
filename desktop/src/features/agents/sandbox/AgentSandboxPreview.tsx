import * as React from "react";
import { Cpu, Loader2, Monitor, MemoryStick, Power, Timer } from "lucide-react";
import { toast } from "sonner";

import { cn } from "@/shared/lib/cn";
import { useNow } from "@/shared/lib/useNow";
import { useUserProfileQuery } from "@/features/profile/hooks";
import { resolveProfileDisplayName } from "@/features/profile/ui/UserProfilePanelUtils";
import { useAgentSandbox } from "./useAgentSandbox";
import { formatSandboxRemaining, isSandboxExpired } from "./sandboxCountdown";
import { SandboxViewerDialog } from "./SandboxViewerDialog";
import { mintViewerUrl } from "./mintViewerUrl";
import { createAgentSandbox, destroyAgentSandbox } from "./sandboxLifecycle";

/**
 * "This agent has a computer" — the preview window on an agent card, in the
 * spirit of Grok Bot's preview-then-full-view. With a live sandbox (kind:48200
 * and no matching kind:48201) it shows the machine and opens the live screen
 * full-size on click; without one it offers "Start a computer", the app-side
 * self-service create.
 *
 * The still frame is a placeholder today; a periodic thumbnail is a later
 * refinement.
 */
export function AgentSandboxPreview({ agentPubkey }: { agentPubkey: string }) {
  const sandbox = useAgentSandbox(agentPubkey);
  const now = useNow(1000);
  const { data: profile } = useUserProfileQuery(agentPubkey);
  const agentDisplayName = resolveProfileDisplayName({
    persona: undefined,
    profile,
    pubkey: agentPubkey,
  });
  const [viewerOpen, setViewerOpen] = React.useState(false);
  const [mintedUrl, setMintedUrl] = React.useState<string | null>(null);
  const [opening, setOpening] = React.useState(false);
  const [starting, setStarting] = React.useState(false);
  const [stopping, setStopping] = React.useState(false);
  const expired =
    sandbox?.expiresAt != null && isSandboxExpired(sandbox.expiresAt, now);

  // The 48200 announcement flips `sandbox` on; until then the button spins.
  React.useEffect(() => {
    if (sandbox) setStarting(false);
    else setStopping(false);
  }, [sandbox]);

  async function startComputer() {
    if (starting) return;
    setStarting(true);
    try {
      if (sandbox && expired) await destroyAgentSandbox(sandbox.id);
      await createAgentSandbox(agentPubkey);
      // Success shows up as the preview itself, via the relay event.
    } catch (err) {
      console.error("[AgentSandboxPreview] create failed:", err);
      toast.error(
        err instanceof Error ? err.message : "Could not start a computer.",
      );
      setStarting(false);
    }
  }

  if (!sandbox || expired) {
    return (
      <div className="mt-2">
        <button
          type="button"
          data-testid={`agent-sandbox-start-${agentPubkey}`}
          disabled={starting}
          onClick={(e) => {
            e.stopPropagation();
            void startComputer();
          }}
          className={cn(
            "flex w-full max-w-sm items-center gap-2 rounded-lg border border-dashed border-border bg-muted/20 p-2 text-left transition-colors",
            starting ? "cursor-default" : "cursor-pointer hover:bg-muted/50",
          )}
        >
          {starting ? (
            <Loader2 className="h-4 w-4 shrink-0 animate-spin text-muted-foreground" />
          ) : (
            <Monitor className="h-4 w-4 shrink-0 text-muted-foreground" />
          )}
          <span className="text-sm text-muted-foreground">
            {starting
              ? "Starting computer…"
              : expired
                ? "Computer expired — start a new one"
                : "Start a computer"}
          </span>
        </button>
        {sandbox ? (
          <button
            type="button"
            data-testid={`agent-sandbox-stop-${agentPubkey}`}
            disabled={stopping || starting}
            onClick={(e) => {
              e.stopPropagation();
              if (stopping) return;
              setStopping(true);
              destroyAgentSandbox(sandbox.id).catch((err) => {
                console.error("[AgentSandboxPreview] destroy failed:", err);
                toast.error(
                  err instanceof Error
                    ? err.message
                    : "Could not stop the computer.",
                );
                setStopping(false);
              });
            }}
            className="mt-1 flex items-center gap-1 text-2xs text-muted-foreground transition-colors hover:text-destructive disabled:opacity-60"
          >
            {stopping ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <Power className="h-3 w-3" />
            )}
            {stopping ? "Stopping…" : "Stop expired computer"}
          </button>
        ) : null}
      </div>
    );
  }

  const remaining =
    sandbox.expiresAt !== null
      ? formatSandboxRemaining(sandbox.expiresAt, now)
      : null;
  const memoryGb =
    sandbox.memoryMb !== null ? (sandbox.memoryMb / 1024).toFixed(1) : null;
  const canOpen = Boolean(sandbox.viewerUrl);

  // Mint a fresh signed link at the instant of opening — the token is only
  // valid briefly, so it must not be minted ahead of time.
  async function openViewer() {
    if (!sandbox?.viewerUrl || opening) return;
    setOpening(true);
    try {
      setMintedUrl(await mintViewerUrl(sandbox.viewerUrl));
      setViewerOpen(true);
    } catch (err) {
      console.error("[AgentSandboxPreview] mint failed:", err);
      toast.error("Could not open the agent's screen. Try again.");
    } finally {
      setOpening(false);
    }
  }

  return (
    <div className="mt-2">
      <button
        type="button"
        data-testid={`agent-sandbox-preview-${agentPubkey}`}
        disabled={!canOpen || opening}
        onClick={(e) => {
          e.stopPropagation();
          if (canOpen) void openViewer();
        }}
        className={cn(
          "group flex w-full max-w-sm items-stretch gap-3 rounded-lg border border-border bg-muted/40 p-2 text-left transition-colors",
          canOpen ? "cursor-pointer hover:bg-muted/70" : "cursor-default",
        )}
      >
        {/* Still-frame preview area. Placeholder until thumbnails land. */}
        <div className="relative flex h-16 w-24 shrink-0 items-center justify-center overflow-hidden rounded-md bg-gradient-to-br from-muted to-muted-foreground/10">
          <Monitor className="h-6 w-6 text-muted-foreground" />
          {canOpen ? (
            <span className="absolute bottom-1 right-1 rounded bg-background/80 px-1 text-3xs font-medium text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
              Open
            </span>
          ) : null}
        </div>

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <span className="truncate text-sm font-medium text-foreground">
              {expired ? "Computer expired" : "Has a computer"}
            </span>
          </div>
          {sandbox.image ? (
            <p className="mt-0.5 truncate text-2xs text-muted-foreground">
              {sandbox.image}
            </p>
          ) : null}
          <div className="mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-2xs text-muted-foreground">
            {sandbox.cpus !== null ? (
              <span className="flex items-center gap-0.5">
                <Cpu className="h-3 w-3" />
                {sandbox.cpus} CPU
              </span>
            ) : null}
            {memoryGb !== null ? (
              <span className="flex items-center gap-0.5">
                <MemoryStick className="h-3 w-3" />
                {memoryGb} GB
              </span>
            ) : null}
            {remaining !== null ? (
              <span
                className={cn(
                  "flex items-center gap-0.5",
                  expired && "text-amber-600 dark:text-amber-400",
                )}
              >
                <Timer className="h-3 w-3" />
                {remaining}
              </span>
            ) : null}
          </div>
          {!sandbox.viewerUrl && !expired ? (
            <p className="mt-1 text-2xs text-muted-foreground/70">
              Screen not reachable
            </p>
          ) : null}
        </div>
      </button>

      <button
        type="button"
        data-testid={`agent-sandbox-stop-${agentPubkey}`}
        disabled={stopping}
        onClick={(e) => {
          e.stopPropagation();
          if (stopping || !sandbox) return;
          setStopping(true);
          destroyAgentSandbox(sandbox.id).catch((err) => {
            console.error("[AgentSandboxPreview] destroy failed:", err);
            toast.error(
              err instanceof Error
                ? err.message
                : "Could not stop the computer.",
            );
            setStopping(false);
          });
        }}
        className="mt-1 flex items-center gap-1 text-2xs text-muted-foreground transition-colors hover:text-destructive disabled:opacity-60"
      >
        {stopping ? (
          <Loader2 className="h-3 w-3 animate-spin" />
        ) : (
          <Power className="h-3 w-3" />
        )}
        {stopping ? "Stopping…" : "Stop computer"}
      </button>

      {mintedUrl ? (
        <SandboxViewerDialog
          open={viewerOpen}
          onOpenChange={setViewerOpen}
          viewerUrl={mintedUrl}
          rawViewerUrl={sandbox.viewerUrl ?? undefined}
          sandboxId={sandbox.id}
          sandboxName={sandbox.name}
          ownerPubkey={agentPubkey}
          agentDisplayName={agentDisplayName}
          remaining={remaining}
          expired={expired}
          expiresAt={sandbox.expiresAt}
        />
      ) : null}
    </div>
  );
}
