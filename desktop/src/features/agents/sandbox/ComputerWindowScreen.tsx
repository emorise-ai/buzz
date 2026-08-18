import * as React from "react";
import { Monitor } from "lucide-react";
import { toast } from "sonner";

import { useUserProfileQuery } from "@/features/profile/hooks";
import { resolveProfileDisplayName } from "@/features/profile/ui/UserProfilePanelUtils";
import { useNow } from "@/shared/lib/useNow";
import { useAgentSandboxById } from "./useAgentSandboxById";
import { formatSandboxRemaining, isSandboxExpired } from "./sandboxCountdown";
import { mintViewerUrl } from "./mintViewerUrl";
import { SandboxStage } from "./SandboxStage";

/**
 * Full-window content for the computer pop-out (`#/computer/:sandboxId`,
 * rendered with no app chrome — see `computerWindowSandboxId` / `root.tsx`).
 * Same live view as the sidebar panel and the fullscreen dialog
 * (`SandboxStage`), just filling the whole native window.
 */
export function ComputerWindowScreen({ sandboxId }: { sandboxId: string }) {
  const found = useAgentSandboxById(sandboxId);
  const now = useNow(1000);
  const { data: profile } = useUserProfileQuery(found?.ownerPubkey);
  const agentDisplayName = found
    ? resolveProfileDisplayName({
        persona: undefined,
        profile,
        pubkey: found.ownerPubkey,
      })
    : null;
  const [userInControl, setUserInControl] = React.useState(false);
  const [mintedUrl, setMintedUrl] = React.useState<string | null>(null);

  const viewerUrl = found?.sandbox.viewerUrl ?? null;

  React.useEffect(() => {
    setMintedUrl(null);
    if (!viewerUrl) return;
    let cancelled = false;
    mintViewerUrl(viewerUrl)
      .then((url) => {
        if (!cancelled) setMintedUrl(url);
      })
      .catch((err) => {
        console.error("[ComputerWindowScreen] mint failed:", err);
        if (!cancelled) {
          toast.error("Could not open the agent's screen. Try again.");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [viewerUrl]);

  if (!found) {
    // Either the sandbox is gone, or the shared sandbox subscription hasn't
    // reported it yet — the window boots through the same community/relay
    // init as the main window, so there's a brief window where this is
    // legitimately still loading rather than truly missing.
    return (
      <div className="flex h-screen w-screen flex-col items-center justify-center gap-2 bg-background p-6 text-center">
        <Monitor className="h-8 w-8 text-muted-foreground" />
        <p className="text-sm font-medium text-foreground">
          Connecting to the agent's computer…
        </p>
      </div>
    );
  }

  const { sandbox } = found;
  const expired =
    sandbox.expiresAt != null && isSandboxExpired(sandbox.expiresAt, now);
  const remaining =
    sandbox.expiresAt != null
      ? formatSandboxRemaining(sandbox.expiresAt, now)
      : null;

  if (!mintedUrl) {
    return (
      <div className="flex h-screen w-screen flex-col items-center justify-center gap-2 bg-background p-6 text-center">
        <Monitor className="h-8 w-8 animate-pulse text-muted-foreground" />
        <p className="text-sm font-medium text-foreground">Connecting…</p>
      </div>
    );
  }

  return (
    <div className="flex h-screen w-screen flex-col bg-background">
      <SandboxStage
        viewerUrl={mintedUrl}
        sandboxId={sandbox.id}
        sandboxName={sandbox.name}
        agentDisplayName={agentDisplayName}
        remaining={remaining}
        expired={expired}
        userInControl={userInControl}
        onUserInControlChange={setUserInControl}
      />
    </div>
  );
}
