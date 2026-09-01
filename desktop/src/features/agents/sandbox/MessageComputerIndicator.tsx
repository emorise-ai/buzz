import { Monitor } from "lucide-react";
import { toast } from "sonner";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { useNow } from "@/shared/lib/useNow";
import { useAgentSandbox } from "./useAgentSandbox";
import { useHasEverHadComputer } from "./computerEverAssignedStore";
import { toggleComputerPanel } from "./computerPanelStore";
import {
  restartAndOpenComputer,
  startAndOpenComputer,
} from "./sandboxLifecycle";
import { isSandboxExpired } from "./sandboxCountdown";

/**
 * Small clickable "this agent has a computer" glyph for the message list,
 * next to an agent author's name (`MessageRow.tsx`). A near-copy of
 * `AgentSandboxCardIndicator`, but interactive: the card indicator sits
 * inside an already-clickable card, while a message row has no equivalent
 * click target, so this one needs its own click/tooltip/aria affordance.
 *
 * Renders nothing for an agent that has never had a computer. Solid when
 * live (click opens the existing preview); dim/outline when the agent has
 * had one before but it's currently gone (click starts a new one and opens
 * the preview) — same three-state contract as the channel header button.
 */
export function MessageComputerIndicator({
  agentPubkey,
}: {
  agentPubkey: string;
}) {
  const sandbox = useAgentSandbox(agentPubkey);
  const now = useNow(1_000);
  const everHadComputer = useHasEverHadComputer(agentPubkey);
  if (!sandbox && !everHadComputer) return null;

  const expired = Boolean(
    sandbox?.expiresAt != null && isSandboxExpired(sandbox.expiresAt, now),
  );
  const isLive = Boolean(sandbox) && !expired;
  const label = isLive ? "Open computer" : "Start computer";

  function handleClick() {
    if (isLive) {
      toggleComputerPanel(agentPubkey);
      return;
    }
    const action =
      expired && sandbox
        ? restartAndOpenComputer(agentPubkey, sandbox.id)
        : startAndOpenComputer(agentPubkey);
    action.catch((err) => {
      console.error(
        "[MessageComputerIndicator] startAndOpenComputer failed:",
        err,
      );
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
          className={
            isLive
              ? "flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-foreground/70 hover:text-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              : "flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-muted-foreground/40 hover:text-muted-foreground/70 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
          }
          data-testid={`message-computer-indicator-${agentPubkey}`}
          onClick={(e) => {
            e.stopPropagation();
            handleClick();
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
