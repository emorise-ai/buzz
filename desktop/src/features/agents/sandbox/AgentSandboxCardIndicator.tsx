import { Monitor } from "lucide-react";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { useAgentSandbox } from "./useAgentSandbox";

/**
 * A glance-level "this agent has a computer" glyph for the agent grid card.
 * Renders nothing unless a live sandbox exists. The full preview and screen
 * viewer live in the agent's profile panel; this is only the discoverable hint
 * that opens it. Follows the preview-then-full-view shape at the card scale.
 */
export function AgentSandboxCardIndicator({
  agentPubkey,
}: {
  agentPubkey: string;
}) {
  const sandbox = useAgentSandbox(agentPubkey);
  if (!sandbox) return null;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          data-testid={`agent-sandbox-indicator-${agentPubkey}`}
          className="flex h-6 w-6 items-center justify-center rounded-full border border-border bg-background/90 text-muted-foreground shadow-xs"
        >
          <Monitor className="h-3.5 w-3.5" />
        </span>
      </TooltipTrigger>
      <TooltipContent>Has a computer — open to view its screen</TooltipContent>
    </Tooltip>
  );
}
