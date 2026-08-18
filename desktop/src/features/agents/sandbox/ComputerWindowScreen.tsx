import * as React from "react";
import { Monitor } from "lucide-react";
import { toast } from "sonner";

import { useUserProfileQuery } from "@/features/profile/hooks";
import { resolveProfileDisplayName } from "@/features/profile/ui/UserProfilePanelUtils";
import { useNow } from "@/shared/lib/useNow";
import { useAgentSandboxById } from "./useAgentSandboxById";
import { formatSandboxRemaining, isSandboxExpired } from "./sandboxCountdown";
import { mintViewerUrl } from "./mintViewerUrl";
import { readComputerWindowHandoff } from "./computerWindow";
import { SandboxStage } from "./SandboxStage";

/** How long to wait for either the handoff params or the shared sandbox
 *  store before giving up on "Connecting…" and showing an actionable error.
 *  The pop-out's own community/relay bootstrap can stall (a second identity
 *  check, a slow relay) even though the parent window already had
 *  everything it needed — an infinite spinner then looks indistinguishable
 *  from "working on it." */
const CONNECT_TIMEOUT_MS = 10_000;

/**
 * Full-window content for the computer pop-out (`#/computer/:sandboxId`,
 * rendered with no app chrome — see `computerWindowSandboxId` / `root.tsx`).
 * Same live view as the sidebar panel and the fullscreen dialog
 * (`SandboxStage`), just filling the whole native window.
 *
 * The pop-out is a second webview that boots its own community/relay init
 * from scratch, so the shared sandbox store (`useAgentSandboxById`) can take
 * a while to repopulate — or never does, if that second bootstrap stalls.
 * The opener already has everything (see `openComputerWindow`), so this
 * prefers the handoff carried in the window's own URL and only falls back
 * to the store for whatever the handoff didn't include.
 */
export function ComputerWindowScreen({ sandboxId }: { sandboxId: string }) {
  const [handoff] = React.useState(readComputerWindowHandoff);
  const found = useAgentSandboxById(sandboxId);
  const now = useNow(1000);

  const ownerPubkey = handoff.ownerPubkey ?? found?.ownerPubkey ?? null;
  const { data: profile } = useUserProfileQuery(ownerPubkey ?? undefined);
  const resolvedDisplayName = ownerPubkey
    ? resolveProfileDisplayName({
        persona: undefined,
        profile,
        pubkey: ownerPubkey,
      })
    : null;
  // The handoff's display name (resolved once, parent-side) is the initial
  // paint; the profile query above refines it once this window's own query
  // cache warms up, same as any other profile-driven name.
  const agentDisplayName =
    resolvedDisplayName ?? handoff.agentDisplayName ?? null;

  const [userInControl, setUserInControl] = React.useState(false);
  const [mintedUrl, setMintedUrl] = React.useState<string | null>(null);
  const [timedOut, setTimedOut] = React.useState(false);

  const viewerUrl = handoff.viewerUrl ?? found?.sandbox.viewerUrl ?? null;
  const sandboxName = handoff.sandboxName ?? found?.sandbox.name ?? null;
  const expiresAt = handoff.expiresAt ?? found?.sandbox.expiresAt ?? null;

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

  // Nothing to connect to yet (no viewer URL from the handoff, and the store
  // hasn't reported the sandbox either) — give the store a bounded window to
  // catch up before treating it as unreachable rather than spinning forever.
  React.useEffect(() => {
    if (viewerUrl) return;
    setTimedOut(false);
    const timer = window.setTimeout(
      () => setTimedOut(true),
      CONNECT_TIMEOUT_MS,
    );
    return () => window.clearTimeout(timer);
  }, [viewerUrl]);

  if (!viewerUrl) {
    if (timedOut) {
      return (
        <div className="flex h-screen w-screen flex-col items-center justify-center gap-2 bg-background p-6 text-center">
          <Monitor className="h-8 w-8 text-muted-foreground" />
          <p className="text-sm font-medium text-foreground">
            Couldn't reach the computer
          </p>
          <p className="max-w-sm text-2xs text-muted-foreground">
            Close this window and reopen it from the agent's chat.
          </p>
        </div>
      );
    }
    return (
      <div className="flex h-screen w-screen flex-col items-center justify-center gap-2 bg-background p-6 text-center">
        <Monitor className="h-8 w-8 text-muted-foreground" />
        <p className="text-sm font-medium text-foreground">
          Connecting to the agent's computer…
        </p>
      </div>
    );
  }

  const expired = expiresAt != null && isSandboxExpired(expiresAt, now);
  const remaining =
    expiresAt != null ? formatSandboxRemaining(expiresAt, now) : null;

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
        sandboxId={sandboxId}
        sandboxName={sandboxName}
        agentDisplayName={agentDisplayName}
        remaining={remaining}
        expired={expired}
        userInControl={userInControl}
        onUserInControlChange={setUserInControl}
      />
    </div>
  );
}
