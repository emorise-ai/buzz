import { invokeTauri } from "@/shared/api/tauri";
import { markComputerEverAssigned } from "./computerEverAssignedStore";
import { openComputerPanel } from "./computerPanelStore";

/**
 * Self-service sandbox lifecycle from the app: "Start computer" / "Stop
 * computer" on an agent. The Tauri backend signs the broker call with the
 * user's key; the sandbox's owner is the agent, so the agent's own
 * `buzz sandbox` commands govern the same machine.
 *
 * Neither call updates UI state directly — the broker announces the change as
 * kind:48200/48201 events the app is already subscribed to, so the preview
 * follows through the normal event path.
 */
export async function createAgentSandbox(agentPubkey: string): Promise<void> {
  await invokeTauri("create_agent_sandbox", { agentPubkey });
  // Mark eagerly rather than waiting for the 48200 round-trip: the caller
  // just started this agent's computer, so "ever had one" is already true.
  markComputerEverAssigned(agentPubkey);
}

export async function destroyAgentSandbox(sandboxId: string): Promise<void> {
  await invokeTauri("destroy_agent_sandbox", { sandboxId });
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
export async function startAndOpenComputer(agentPubkey: string): Promise<void> {
  openComputerPanel(agentPubkey);
  await createAgentSandbox(agentPubkey);
}
