/**
 * Derive a sandbox's terminal URL from its viewer URL. The broker exposes the
 * live desktop at a URL ending in `/desktop`; the terminal lives at the same
 * path with `/desktop` swapped for `/terminal`. Both are minted the same way
 * (a NIP-98 GET token signed over the exact URL) — this only computes the
 * unsigned address.
 */
export function deriveTerminalUrl(viewerUrl: string): string | null {
  // Tolerate an already-minted URL (`…/desktop?t=…`): the token is bound to
  // the desktop URL and useless for the terminal, so strip the query and
  // derive from the bare address. The caller mints a terminal-specific
  // token afterwards.
  const bare = viewerUrl.split("?", 1)[0] ?? "";
  if (!/\/desktop$/.test(bare)) return null;
  return bare.replace(/\/desktop$/, "/terminal");
}
