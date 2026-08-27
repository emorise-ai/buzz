import * as React from "react";
import { Loader2, TerminalSquare } from "lucide-react";

import { deriveTerminalUrl } from "./terminalUrl";
import { mintViewerUrl } from "./mintViewerUrl";

/**
 * The Terminal view. Minted once, on first activation, then kept mounted for
 * the life of the workspace — remounting the iframe would drop the terminal's
 * connection the same way it would for the VNC screen.
 */
export function SandboxTerminalView({
  viewerUrl,
  active,
}: {
  viewerUrl: string | null;
  active: boolean;
}) {
  const [mintedUrl, setMintedUrl] = React.useState<string | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(false);
  const started = React.useRef(false);

  React.useEffect(() => {
    if (!active || started.current || !viewerUrl) return;
    const terminalUrl = deriveTerminalUrl(viewerUrl);
    if (!terminalUrl) {
      setError("This sandbox does not expose a terminal.");
      return;
    }
    started.current = true;
    setLoading(true);
    mintViewerUrl(terminalUrl)
      .then((url) => setMintedUrl(url))
      .catch((err) => {
        started.current = false;
        setError(
          err instanceof Error ? err.message : "Could not open the terminal.",
        );
      })
      .finally(() => setLoading(false));
  }, [active, viewerUrl]);

  if (!viewerUrl) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
        <TerminalSquare className="h-8 w-8 text-muted-foreground" />
        <p className="text-sm font-medium text-foreground">
          No terminal available
        </p>
        <p className="max-w-sm text-2xs text-muted-foreground">
          This computer has no live screen to attach a terminal to.
        </p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
        <TerminalSquare className="h-8 w-8 text-muted-foreground" />
        <p className="text-sm font-medium text-foreground">
          Could not open the terminal
        </p>
        <p className="max-w-sm text-2xs text-muted-foreground">{error}</p>
      </div>
    );
  }

  if (!mintedUrl) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground">
        {loading ? <Loader2 className="h-5 w-5 animate-spin" /> : null}
      </div>
    );
  }

  return (
    <iframe
      src={mintedUrl}
      title="Sandbox terminal"
      className="h-full w-full border-0 bg-black"
      sandbox="allow-scripts allow-same-origin allow-forms"
    />
  );
}
