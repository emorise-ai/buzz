import * as React from "react";

import type { AgentSandbox } from "./sandboxState";
import { getSandboxForOwner, subscribeSandboxStore } from "./agentSandboxStore";

/**
 * Live "does this agent have a computer?" state for one agent, read from the
 * shared agent-sandbox store (a single community-wide subscription fanned out by
 * owner). Reading through the store — instead of opening a subscription per card
 * — keeps the app well under the relay's per-connection subscription quota.
 */
export function useAgentSandbox(
  agentPubkey: string | null,
): AgentSandbox | null {
  const getSnapshot = React.useCallback(
    () => getSandboxForOwner(agentPubkey),
    [agentPubkey],
  );
  return React.useSyncExternalStore(subscribeSandboxStore, getSnapshot);
}
