import { createFileRoute } from "@tanstack/react-router";

import { ComputerWindowScreen } from "@/features/agents/sandbox/ComputerWindowScreen";

/**
 * In practice, `root.tsx` intercepts `computerWindowSandboxId()` and renders
 * `ComputerWindowScreen` directly before this route's component ever mounts
 * — the pop-out window is a dedicated webview labeled `computer-<id>`, and
 * that check happens above the router. This route component is the
 * fallback for the (currently unreachable) case where a `/computer/:id`
 * URL is hit inside a normal, non-pop-out window; it renders the same
 * screen rather than a stub, so there is exactly one implementation of
 * "show this sandbox full-window," not two drifting copies.
 */
export const Route = createFileRoute("/computer/$sandboxId")({
  component: ComputerWindowRoute,
});

function ComputerWindowRoute() {
  const { sandboxId } = Route.useParams();
  return <ComputerWindowScreen sandboxId={sandboxId} />;
}
