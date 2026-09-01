import { Monitor } from "lucide-react";
import { toast } from "sonner";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { useNow } from "@/shared/lib/useNow";
import { cn } from "@/shared/lib/cn";
import { useHasEverHadComputer } from "./computerEverAssignedStore";
import { createAgentSandbox, restartComputer } from "./sandboxLifecycle";
import { isSandboxExpired } from "./sandboxCountdown";
import { openComputerWindow } from "./openComputerWindow";
import { useAgentSandbox } from "./useAgentSandbox";

/**
 * Direct computer control on an agent grid card. It is a sibling of the card's
 * full-size overlay button, so it can be a real button without invalid nested
 * interactive markup; stopping propagation keeps it from opening the profile.
 */
export function AgentSandboxCardIndicator({
  agentPubkey,
}: {
  agentPubkey: string;
}) {
  const sandbox = useAgentSandbox(agentPubkey);
  const everHadComputer = useHasEverHadComputer(agentPubkey);
  const now = useNow(1_000);
  if (!sandbox && !everHadComputer) return null;

  const expired = Boolean(
    sandbox?.expiresAt != null && isSandboxExpired(sandbox.expiresAt, now),
  );
  const isLive = Boolean(sandbox) && !expired;
  const label = isLive ? "Open computer" : "Start computer";

  async function handleClick() {
    if (isLive) {
      if (!sandbox) return;
      await openComputerWindow(sandbox.id, "Agent's computer", {
        viewerUrl: sandbox.viewerUrl,
        sandboxName: sandbox.name,
        ownerPubkey: agentPubkey,
        expiresAt: sandbox.expiresAt,
      });
      return;
    }
    const created = await (expired && sandbox
      ? restartComputer(agentPubkey, sandbox.id)
      : createAgentSandbox(agentPubkey));
    if (created) {
      await openComputerWindow(created.id, "Agent's computer", {
        viewerUrl: created.viewerUrl,
        sandboxName: created.name,
        ownerPubkey: agentPubkey,
        expiresAt: created.expiresAt,
      });
    }
  }

  function runComputerAction() {
    handleClick().catch((err) => {
      console.error("[AgentSandboxCardIndicator] computer action failed:", err);
      toast.error(
        err instanceof Error ? err.message : "Could not start a computer.",
      );
    });
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          aria-label={label}
          data-testid={`agent-sandbox-indicator-${agentPubkey}`}
          className={cn(
            "flex h-6 w-6 items-center justify-center rounded-full border border-border bg-background/90 shadow-xs focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
            isLive
              ? "text-foreground/70 hover:text-foreground"
              : "text-muted-foreground/40 hover:text-muted-foreground/70",
          )}
          onClick={(event) => {
            event.stopPropagation();
            runComputerAction();
          }}
          type="button"
        >
          <Monitor className="h-3.5 w-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
