import { invokeTauri } from "@/shared/api/tauri";

/**
 * Turn the relay-announced viewer URL into an openable one by minting a signed,
 * short-lived access token for it. The browser cannot sign the request the way
 * the app's own HTTP calls do, so the Tauri backend signs a NIP-98 GET over the
 * exact URL with the user's key and returns `<url>?t=<token>`; the broker
 * verifies it before proxying the screen.
 *
 * The token is fresh only briefly (the relay's ±60s NIP-98 window), so this is
 * called at the moment the user opens the screen, not ahead of time.
 */
export async function mintViewerUrl(viewerUrl: string): Promise<string> {
  return invokeTauri<string>("mint_sandbox_viewer_url", { viewerUrl });
}
