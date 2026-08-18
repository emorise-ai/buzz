import { createRootRoute } from "@tanstack/react-router";

import { AppShell } from "@/app/AppShell";
import { computerWindowSandboxId } from "@/features/agents/sandbox/computerWindow";
import { ComputerWindowScreen } from "@/features/agents/sandbox/ComputerWindowScreen";

/**
 * The root route normally renders the full app shell (sidebar, header,
 * channel pane). A computer pop-out window is a second webview booted
 * through the same community/relay init (see `open_computer_window` /
 * `computerWindowSandboxId`), but it has nothing to do with channels or
 * navigation — it exists to show one sandbox's live screen full-window with
 * no chrome. Intercepting here, before `AppShell` mounts, skips the entire
 * sidebar/header subtree rather than rendering it hidden.
 */
function RootRoute() {
  const sandboxId = computerWindowSandboxId();
  if (sandboxId !== null) {
    return <ComputerWindowScreen sandboxId={sandboxId} />;
  }
  return <AppShell />;
}

export const Route = createRootRoute({
  component: RootRoute,
});
