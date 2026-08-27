import * as React from "react";

import type { AgentSandbox } from "./sandboxState";
import { getSandboxById, subscribeSandboxStore } from "./agentSandboxStore";

/**
 * Live "does this sandbox id still exist?" lookup, for surfaces that only
 * know a sandbox id — the pop-out window route, opened with just the id from
 * its window label. See `useAgentSandbox` for the owner-keyed sibling most
 * call sites want instead.
 */
export function useAgentSandboxById(
  sandboxId: string | null,
): { sandbox: AgentSandbox; ownerPubkey: string } | null {
  const getSnapshot = React.useCallback(
    () => getSandboxById(sandboxId),
    [sandboxId],
  );
  return React.useSyncExternalStore(subscribeSandboxStore, getSnapshot);
}
