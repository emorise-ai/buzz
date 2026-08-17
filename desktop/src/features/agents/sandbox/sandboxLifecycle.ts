import { invokeTauri } from "@/shared/api/tauri";

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
}

export async function destroyAgentSandbox(sandboxId: string): Promise<void> {
  await invokeTauri("destroy_agent_sandbox", { sandboxId });
}
