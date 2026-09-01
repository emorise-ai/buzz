import { invokeTauri } from "@/shared/api/tauri";
import { markComputerEverAssigned } from "./computerEverAssignedStore";
import { removeSandboxById, upsertSandboxForOwner } from "./agentSandboxStore";
import { openComputerPanel } from "./computerPanelStore";
import { sandboxFromCreateResponse } from "./sandboxState";
import type { AgentSandbox } from "./sandboxState";

/**
 * Self-service sandbox lifecycle from the app: "Start computer" / "Stop
 * computer" on an agent. The Tauri backend signs the broker call with the
 * user's key; the sandbox's owner is the agent, so the agent's own
 * `buzz sandbox` commands govern the same machine.
 *
 * Successful command responses update the local store immediately, then the
 * broker's kind:48200/48201 events reconcile that state through the normal
 * durable event path. This keeps the UI honest when an event is delayed or an
 * idempotent stop finds that the machine is already gone.
 */
export async function createAgentSandbox(
  agentPubkey: string,
): Promise<AgentSandbox | null> {
  const response = await invokeTauri<unknown>("create_agent_sandbox", {
    agentPubkey,
  });
  const sandbox = sandboxFromCreateResponse(response);
  if (sandbox) upsertSandboxForOwner(agentPubkey, sandbox);
  // Mark eagerly rather than waiting for the 48200 round-trip: the caller
  // just started this agent's computer, so "ever had one" is already true.
  markComputerEverAssigned(agentPubkey);
  return sandbox;
}

export async function destroyAgentSandbox(sandboxId: string): Promise<void> {
  await invokeTauri("destroy_agent_sandbox", { sandboxId });
  removeSandboxById(sandboxId);
}

/**
 * "Start a new computer and show it" — the click behavior for a dim/asleep
 * computer affordance (header button, message-list indicator) once an agent
 * has had a computer before but it's currently gone. Opens the panel first
 * so the user sees the "starting" state immediately, then kicks off the
 * create call; the panel's own empty state carries the wait until the 48200
 * event flips `useAgentSandbox` truthy. Throws on failure — callers surface
 * that via a toast, matching `AgentSandboxPreview`'s existing pattern.
 */
export async function startAndOpenComputer(
  agentPubkey: string,
): Promise<AgentSandbox | null> {
  openComputerPanel(agentPubkey);
  return createAgentSandbox(agentPubkey);
}

/** Replace an expired/stale machine, preserving its durable volumes. */
export async function restartAndOpenComputer(
  agentPubkey: string,
  sandboxId: string,
): Promise<AgentSandbox | null> {
  openComputerPanel(agentPubkey);
  return restartComputer(agentPubkey, sandboxId);
}

/** Replace an expired/stale machine without choosing a presentation surface. */
export async function restartComputer(
  agentPubkey: string,
  sandboxId: string,
): Promise<AgentSandbox | null> {
  await destroyAgentSandbox(sandboxId);
  return createAgentSandbox(agentPubkey);
}
