//! Minting a signed, short-lived link to an agent sandbox's live desktop.
//!
//! The sandbox viewer is a logged-in browser desktop, so its screen must not be
//! openable by anyone who learns the URL. A plain browser opening an `<iframe>`
//! cannot sign a request the way the app's HTTP calls do, so the desktop mints
//! the credential here — a NIP-98 event signed with the user's key over the
//! exact viewer URL — and hands the browser a URL carrying it as `?t=<token>`.
//! The broker verifies the token (signature, freshness, community membership)
//! before it proxies a single byte of the screen.
//!
//! The token is the same base64-encoded NIP-98 event the relay's HTTP surface
//! accepts, minus the `Nostr ` scheme prefix — one identity system, not two.

use base64::Engine;
use reqwest::Method;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::app_state::AppState;
use crate::relay::{build_nip98_auth_header, build_nip98_auth_header_for_hash};

const SANDBOX_FILE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

type CmdResult<T> = Result<T, String>;

/// Re-encode the signed NIP-98 event as URL-safe base64.
///
/// The Authorization header form uses standard base64 (`+` `/` `=`), but the
/// token rides in a query string where those characters are mangled — `+`
/// decodes to a space under form-urlencoding. base64url is URL-safe end to end,
/// so it needs no escaping and the broker reads it back verbatim.
fn to_base64url(standard_b64: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(standard_b64.trim())
        .map_err(|_| "signed token was not valid base64".to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Mint a viewer URL that carries a signed, short-lived access token.
///
/// `viewer_url` is the address the relay announced on the sandbox's kind:48200
/// event (`viewer` tag). The token is a NIP-98 GET signature over that exact
/// URL, so the broker — which reconstructs the same URL from the request's
/// forwarded host — can verify it without the app asserting anything new.
#[tauri::command]
pub fn mint_sandbox_viewer_url(
    viewer_url: String,
    state: State<'_, AppState>,
) -> CmdResult<String> {
    let trimmed = viewer_url.trim();
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("viewer URL must be http(s)".to_string());
    }
    if trimmed.contains('?') {
        // The token is signed over the bare URL; a pre-existing query string
        // would change what the broker verifies against.
        return Err("viewer URL must not already carry a query string".to_string());
    }

    // NIP-98 signs the exact URL, so the token is bound to this sandbox's
    // screen and cannot be replayed against another.
    let header = build_nip98_auth_header(&Method::GET, trimmed, &[], &state)?;
    let token = header
        .strip_prefix("Nostr ")
        .ok_or_else(|| "unexpected auth header format".to_string())?;

    Ok(format!("{trimmed}?t={}", to_base64url(token)?))
}

// ── Self-service sandbox lifecycle ──────────────────────────────────────────
//
// "Give this agent a computer" from the app. The desktop reaches the broker
// through the same public reverse-proxy path that serves the viewer, signing
// with the *user's* key — the person managing the agent — while the sandbox's
// owner is the agent, so the agent's own `buzz sandbox` calls govern it too.

/// The fixed self-service profile. One image, modest budget, short default
/// TTL: an agent (or a click in the app) gets a desktop, not a menu of
/// hardware. The broker clamps everything again on its side regardless.
const SELF_SERVICE_IMAGE: &str = "buzz-sprig-desktop";
const SELF_SERVICE_TTL_SECONDS: u64 = 1800;
const SELF_SERVICE_MEMORY_MB: u64 = 8192;

/// Path prefix the relay's reverse proxy forwards to the broker.
const BROKER_PROXY_PREFIX: &str = "/sandbox-viewer";

/// Where the app reaches the broker: an explicit override, else the active
/// community relay's origin plus the viewer proxy prefix — the same door the
/// published viewer links already go through.
///
/// Shared with `managed_agents::runtime`, which stamps this exact value into
/// a spawned agent's `BUZZ_SANDBOX_BROKER_URL` — the two call sites must never
/// drift, or an agent's `buzz sandbox` calls would sign against a URL the
/// broker's `BUZZ_SANDBOX_PUBLIC_URL` allowlist rejects.
pub(crate) fn broker_base(state: &AppState) -> String {
    if let Ok(v) = std::env::var("BUZZ_SANDBOX_BROKER_URL") {
        let v = v.trim();
        if !v.is_empty() {
            return v.trim_end_matches('/').to_string();
        }
    }
    let ws = crate::relay::relay_ws_url_with_override(state);
    format!(
        "{}{BROKER_PROXY_PREFIX}",
        crate::relay::relay_http_base_url(&ws)
    )
}

fn is_hex_pubkey(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn self_service_create_body(agent_pubkey: &str) -> serde_json::Value {
    serde_json::json!({
        "image": SELF_SERVICE_IMAGE,
        "owner": agent_pubkey,
        "ttl_seconds": SELF_SERVICE_TTL_SECONDS,
        "memory_mb": SELF_SERVICE_MEMORY_MB,
        "env": {
            "BUZZ_DEV_MCP_BIND": "0.0.0.0:9320",
            "BUZZ_DEV_MCP_OWNER": agent_pubkey,
            "BUZZ_DESKTOP_ENABLED": "1",
        },
    })
}

/// Create a sandbox owned by `agent_pubkey` — the app-side "Start computer".
///
/// Returns the broker's create response. The UI does not need it to render:
/// the broker announces the sandbox as a kind:48200 the app is already
/// subscribed to, so the preview appears through the normal event path.
///
/// If `agent_pubkey` is a locally-managed agent, this also stamps the new
/// sandbox id onto its record and restarts it — an agent's env is fixed at
/// spawn, so attaching a computer requires a fresh process to pick up
/// `BUZZ_SANDBOX_BROKER_URL`/`BUZZ_SANDBOX_ID` and the harness's "you have a
/// computer" briefing. If the agent isn't managed here (e.g. running on
/// another machine), the box is still created and viewable — it just isn't
/// agent-driven from this desktop, matching today's behavior for such agents.
#[tauri::command]
pub async fn create_agent_sandbox(
    agent_pubkey: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> CmdResult<serde_json::Value> {
    if !is_hex_pubkey(&agent_pubkey) {
        return Err("agent pubkey must be 64 hex characters".to_string());
    }
    let url = format!("{}/sandboxes", broker_base(&state));
    // Tools-only mode: a long-lived tools server plus a visible screen, no
    // second agent harness inside. The in-sandbox tools server refuses to
    // serve without knowing its owner.
    let body = self_service_create_body(&agent_pubkey);
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let auth = build_nip98_auth_header(&Method::POST, &url, &bytes, &state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(broker_error(status.as_u16(), &text));
    }
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("broker returned unparseable JSON: {e}"))?;

    // The sandbox exists (id known) before we touch the agent record, so a
    // freshly-spawned agent always gets a valid id to attach to.
    if let Some(sandbox_id) = parsed.get("id").and_then(|v| v.as_str()) {
        // Creation already succeeded remotely. A local record/restamp failure
        // must not turn that success into a misleading "Start failed" in the
        // UI or discard the response needed to show and stop the new PC.
        if let Err(error) = attach_sandbox_to_local_agent(&app, &agent_pubkey, sandbox_id).await {
            eprintln!(
                "buzz-desktop: sandbox {sandbox_id} was created for agent {agent_pubkey}, but local attachment failed: {error}"
            );
        }
    } else {
        eprintln!(
            "buzz-desktop: sandbox broker create response had no string `id`; agent {agent_pubkey} was not attached"
        );
    }

    Ok(parsed)
}

/// Stamp `sandbox_id` onto `agent_pubkey`'s managed-agent record, if one
/// exists on this machine, and restart it so the new env takes effect.
///
/// Silently no-ops (not an error) when the agent isn't locally managed — the
/// sandbox is still created and viewable, just not agent-driven from here.
async fn attach_sandbox_to_local_agent(
    app: &AppHandle,
    agent_pubkey: &str,
    sandbox_id: &str,
) -> CmdResult<()> {
    let relay_url = restamp_sandbox_id(app, agent_pubkey, Some(sandbox_id.to_string()))?;
    let Some(relay_url) = relay_url else {
        return Ok(());
    };
    if let Err(error) = crate::managed_agents::restart_managed_agent_runtime(
        agent_pubkey.to_string(),
        relay_url,
        app.clone(),
    )
    .await
    {
        eprintln!(
            "buzz-desktop: sandbox {sandbox_id} attached to agent {agent_pubkey}, but the restart to pick it up failed: {error}"
        );
    }
    Ok(())
}

/// Find `pubkey`'s managed-agent record, set `sandbox_id`, and persist.
///
/// Returns `Ok(Some(relay_url))` when a record was found and updated (the
/// caller should restart that agent next), `Ok(None)` when no local record
/// exists for this pubkey (nothing to restart).
fn restamp_sandbox_id(
    app: &AppHandle,
    pubkey: &str,
    sandbox_id: Option<String>,
) -> CmdResult<Option<String>> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let _store = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    let mut records = crate::managed_agents::load_managed_agents(app)?;
    let Some(record) = records.iter_mut().find(|r| r.pubkey == pubkey) else {
        return Ok(None);
    };
    record.sandbox_id = sandbox_id;
    record.updated_at = crate::util::now_iso();
    let relay_url = record.relay_url.clone();
    crate::managed_agents::save_managed_agents(app, &records)?;
    Ok(Some(relay_url))
}

/// Bump a sandbox's expiry — the viewer's passive-watching keepalive.
///
/// `SandboxStage` calls this on an interval while its screen is mounted and
/// visible, so a human just watching (no clicks) keeps the computer alive the
/// same way real activity does. Mirrors `destroy_agent_sandbox`'s request
/// shape (same `broker_base`, `build_nip98_auth_header`, error handling) with
/// no request body — the broker resolves the id and clamps the new expiry
/// itself; the app has no expiry math to get right here.
///
/// Owner-or-manager gated on the broker side. A sandbox that's already gone
/// answers 404, which this surfaces as an ordinary error — callers treat a
/// failed heartbeat as best-effort and must not toast or crash the view.
#[tauri::command]
pub async fn sandbox_heartbeat(sandbox_id: String, state: State<'_, AppState>) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let url = format!("{}/sandboxes/{id}/heartbeat", broker_base(&state));
    let auth = build_nip98_auth_header(&Method::POST, &url, &[], &state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

/// Start capturing a sandbox's screen — the "Teach a task" entry point.
/// Owner-or-manager gated on the broker side, same as every other
/// computer-use call here. A 409 (a recording is already in progress)
/// surfaces as an ordinary error string for the caller to show.
#[tauri::command]
pub async fn sandbox_recording_start(
    sandbox_id: String,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let url = format!("{}/sandboxes/{id}/recording/start", broker_base(&state));
    let auth = build_nip98_auth_header(&Method::POST, &url, &[], &state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

/// Stop an in-progress recording and return the finished mp4's raw bytes.
/// The frontend hands these straight to `uploadMediaBytes` (the same path
/// used for pasted/dragged media) to turn them into a fetchable URL — this
/// command only fetches the bytes, it does not host them anywhere itself.
///
/// `audio` is the "Teach a task" mic capture, already encoded to one of the
/// broker's allowed containers (`audio_ext` names it — see the broker's
/// `/recording/stop` handler). When present, the broker bakes it into the
/// returned mp4 as its audio track instead of returning a silent video.
/// Passing `audio: None` (or omitting it) is the original video-only path,
/// unchanged.
///
/// The query string carrying `audio_ext` is part of the authorized request
/// once audio is attached, so it MUST be included in the exact URL string
/// passed to `build_nip98_auth_header` — signing the bare path and appending
/// the query after the fact would let the broker compute a different NIP-98
/// `u` tag than what was actually requested and 401 the call. Likewise the
/// body hash covers the raw audio bytes, not an empty body, whenever audio is
/// sent.
#[tauri::command]
pub async fn sandbox_recording_stop(
    sandbox_id: String,
    audio: Option<Vec<u8>>,
    audio_ext: Option<String>,
    state: State<'_, AppState>,
) -> CmdResult<Vec<u8>> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let base_url = format!("{}/sandboxes/{id}/recording/stop", broker_base(&state));
    let body = audio.unwrap_or_default();
    let url = if body.is_empty() {
        base_url
    } else {
        let ext = audio_ext
            .filter(|e| !e.is_empty())
            .ok_or("audio_ext is required when audio bytes are provided".to_string())?;
        format!("{base_url}?audio_ext={ext}")
    };
    let auth = build_nip98_auth_header(&Method::POST, &url, &body, &state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("could not read the recording: {e}"))?;
    Ok(bytes.to_vec())
}

/// Best-effort, fire-and-forget recording stop for a sandbox whose viewer
/// window is going away right now (native window close, see `lib.rs`'s
/// `CloseRequested` handling for `computer-*` labels).
///
/// This is the same bare-stop request `sandbox_recording_stop` makes when
/// called with no audio (empty body, no `audio_ext` query param) — it exists
/// so the native window-close path can reuse that exact signing/request shape
/// without going through the Tauri command (which returns the recording
/// bytes; the window is closing, so there is nowhere to put them). Errors are
/// swallowed by the caller: if there was no recording in progress the broker
/// answers 409, and either way the window must not be blocked from closing
/// while this runs.
pub(crate) async fn stop_recording_best_effort(
    sandbox_id: &str,
    state: &AppState,
) -> CmdResult<()> {
    let id = validate_sandbox_id(sandbox_id)?;
    let url = format!("{}/sandboxes/{id}/recording/stop", broker_base(state));
    let auth = build_nip98_auth_header(&Method::POST, &url, &[], state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .body(Vec::new())
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

/// Probe whether a sandbox is still alive on the broker (`GET
/// /sandboxes/{id}`, unauthenticated-by-ownership — any NIP-98-valid caller
/// may read status). Used at agent-start to decide whether a record's stored
/// `sandbox_id` still refers to a live computer before handing it to a freshly
/// spawned agent.
///
/// Returns `Ok(true)` on a 2xx (alive), `Ok(false)` on a definitive 404
/// (gone). Any other failure (network error, non-404 error status) is an
/// `Err` — the caller's contract is to treat that as "unknown, don't touch
/// the id," never as "gone."
pub(crate) async fn sandbox_is_alive(sandbox_id: &str, state: &AppState) -> CmdResult<bool> {
    let id = validate_sandbox_id(sandbox_id)?;
    let url = format!("{}/sandboxes/{id}", broker_base(state));
    let auth = build_nip98_auth_header(&Method::GET, &url, &[], state)?;
    let response = state
        .http_client
        .get(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(true);
    }
    if status.as_u16() == 404 {
        return Ok(false);
    }
    let text = response.text().await.unwrap_or_default();
    Err(broker_error(status.as_u16(), &text))
}

/// Destroy a sandbox now — the app-side "Stop computer". The broker announces
/// the destruction (kind:48201), which clears the preview.
///
/// If a locally-managed agent had this sandbox attached, this also clears its
/// `sandbox_id` and restarts it, so it drops the now-dead broker env and
/// correctly reports having no computer — rather than holding a dead id and
/// having its `buzz sandbox` calls fail against a box that no longer exists.
#[tauri::command]
pub async fn destroy_agent_sandbox(
    sandbox_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> CmdResult<()> {
    let id = sandbox_id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err("malformed sandbox id".to_string());
    }
    let url = format!("{}/sandboxes/{id}", broker_base(&state));
    let auth = build_nip98_auth_header(&Method::DELETE, &url, &[], &state)?;
    let response = state
        .http_client
        .delete(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }

    detach_sandbox_from_local_agent(&app, id).await;
    Ok(())
}

/// Find whichever locally-managed agent has `sandbox_id` attached, clear it,
/// and restart that agent. A no-op (not an error, and not surfaced to the
/// caller) when no local record has this sandbox attached — the box may
/// belong to an agent unmanaged on this machine, or to no agent at all.
async fn detach_sandbox_from_local_agent(app: &AppHandle, sandbox_id: &str) {
    let pubkey = match find_agent_pubkey_for_sandbox(app, sandbox_id) {
        Ok(Some(pubkey)) => pubkey,
        Ok(None) => return,
        Err(error) => {
            eprintln!(
                "buzz-desktop: sandbox {sandbox_id} destroyed, but looking up its attached agent failed: {error}"
            );
            return;
        }
    };
    match restamp_sandbox_id(app, &pubkey, None) {
        Ok(Some(relay_url)) => {
            if let Err(error) = crate::managed_agents::restart_managed_agent_runtime(
                pubkey.clone(),
                relay_url,
                app.clone(),
            )
            .await
            {
                eprintln!(
                    "buzz-desktop: sandbox {sandbox_id} detached from agent {pubkey}, but the restart to drop it failed: {error}"
                );
            }
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!(
                "buzz-desktop: sandbox {sandbox_id} destroyed, but clearing it from agent {pubkey} failed: {error}"
            );
        }
    }
}

/// Find the pubkey of the locally-managed agent whose record currently has
/// `sandbox_id` attached, if any.
fn find_agent_pubkey_for_sandbox(app: &AppHandle, sandbox_id: &str) -> CmdResult<Option<String>> {
    let records = crate::managed_agents::load_managed_agents(app)?;
    Ok(records
        .into_iter()
        .find(|r| r.sandbox_id.as_deref() == Some(sandbox_id))
        .map(|r| r.pubkey))
}

/// Surface the broker's `{"error": "..."}` text rather than the JSON wrapper.
fn broker_error(status: u16, body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
        .unwrap_or_else(|| format!("sandbox broker returned {status}: {body}"))
}

// ── Sandbox file browser ────────────────────────────────────────────────────
//
// The Files view in the workspace UI talks to the broker's `/fs*` endpoints,
// NIP-98-headed the same way as create/destroy. Every call is scoped to one
// sandbox id and one absolute path under `/workspace` or `/home/agent` — the
// broker enforces that boundary; the app only validates shape.

fn validate_sandbox_id(sandbox_id: &str) -> Result<&str, String> {
    let id = sandbox_id.trim();
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err("malformed sandbox id".to_string());
    }
    Ok(id)
}

fn validate_sandbox_path(path: &str) -> Result<&str, String> {
    let path = path.trim();
    if !path.starts_with("/workspace") && !path.starts_with("/home/agent") {
        return Err("path must be absolute under /workspace or /home/agent".to_string());
    }
    Ok(path)
}

/// Percent-encode a path for use as a query parameter value.
fn encode_query_value(value: &str) -> String {
    const FRAGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(value, FRAGMENT).to_string()
}

/// List a directory inside a sandbox. Returns the broker's parsed JSON
/// (`{"path", "entries": [...]}`) unmodified — the frontend owns display
/// formatting (sizes, relative mtimes).
#[tauri::command]
pub async fn sandbox_fs_list(
    sandbox_id: String,
    path: String,
    state: State<'_, AppState>,
) -> CmdResult<serde_json::Value> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let path = validate_sandbox_path(&path)?;
    let url = format!(
        "{}/sandboxes/{id}/fs?path={}",
        broker_base(&state),
        encode_query_value(path)
    );
    let auth = build_nip98_auth_header(&Method::GET, &url, &[], &state)?;
    let response = state
        .http_client
        .get(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(broker_error(status.as_u16(), &text));
    }
    serde_json::from_str(&text).map_err(|e| format!("broker returned unparseable JSON: {e}"))
}

/// Download a file from a sandbox into the user's Downloads directory.
/// Returns the saved local path.
#[tauri::command]
pub async fn sandbox_fs_download(
    sandbox_id: String,
    path: String,
    state: State<'_, AppState>,
) -> CmdResult<String> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let path = validate_sandbox_path(&path)?;
    let url = format!(
        "{}/sandboxes/{id}/fs/file?path={}",
        broker_base(&state),
        encode_query_value(path)
    );
    let auth = build_nip98_auth_header(&Method::GET, &url, &[], &state)?;
    let response = state
        .http_client
        .get(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    let downloads_dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .ok_or_else(|| "could not determine the Downloads directory".to_string())?;
    std::fs::create_dir_all(&downloads_dir)
        .map_err(|e| format!("could not create the Downloads directory: {e}"))?;

    let file_name = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let dest = unique_destination(&downloads_dir, file_name);
    if response
        .content_length()
        .is_some_and(|size| size > SANDBOX_FILE_MAX_BYTES)
    {
        return Err("file is larger than the 1 GiB transfer limit".to_string());
    }
    let temp = tempfile::NamedTempFile::new_in(&downloads_dir)
        .map_err(|e| format!("could not create the download file: {e}"))?;
    let clone = temp
        .reopen()
        .map_err(|e| format!("could not open the download file: {e}"))?;
    let mut output = tokio::fs::File::from_std(clone);
    use tokio::io::AsyncWriteExt;
    let mut response = response;
    let mut size = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("could not read the downloaded file: {e}"))?
    {
        size = size.saturating_add(chunk.len() as u64);
        if size > SANDBOX_FILE_MAX_BYTES {
            return Err("file is larger than the 1 GiB transfer limit".to_string());
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|e| format!("could not save the file: {e}"))?;
    }
    output
        .flush()
        .await
        .map_err(|e| format!("could not finish saving the file: {e}"))?;
    drop(output);
    temp.persist(&dest)
        .map_err(|e| format!("could not save the file: {}", e.error))?;

    Ok(dest.to_string_lossy().to_string())
}

/// Pick a destination path that does not clobber an existing file, appending
/// ` (1)`, ` (2)`, … before the extension the way desktop file managers do.
fn unique_destination(dir: &std::path::Path, file_name: &str) -> std::path::PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());
    for n in 1..10_000 {
        let candidate_name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(&candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(file_name)
}

/// Upload a local file into a sandbox at `dest_path`.
#[tauri::command]
pub async fn sandbox_fs_upload(
    sandbox_id: String,
    local_path: String,
    dest_path: String,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let dest_path = validate_sandbox_path(&dest_path)?;
    let mut file = tokio::fs::File::open(&local_path)
        .await
        .map_err(|e| format!("could not read the local file: {e}"))?;
    let size = file
        .metadata()
        .await
        .map_err(|e| format!("could not inspect the local file: {e}"))?
        .len();
    if size > SANDBOX_FILE_MAX_BYTES {
        return Err("file is larger than the 1 GiB transfer limit".to_string());
    }
    let url = format!(
        "{}/sandboxes/{id}/fs/file?path={}",
        broker_base(&state),
        encode_query_value(dest_path)
    );
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("could not hash the local file: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    file.rewind()
        .await
        .map_err(|e| format!("could not rewind the local file: {e}"))?;
    let auth = build_nip98_auth_header_for_hash(
        &Method::PUT,
        &url,
        &hex::encode(hasher.finalize()),
        &state,
    )?;
    let response = state
        .http_client
        .put(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", size)
        .body(reqwest::Body::wrap_stream(
            tokio_util::io::ReaderStream::new(file),
        ))
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

/// Rename or move a file/directory inside a sandbox.
#[tauri::command]
pub async fn sandbox_fs_rename(
    sandbox_id: String,
    from: String,
    to: String,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let from = validate_sandbox_path(&from)?;
    let to = validate_sandbox_path(&to)?;
    let url = format!("{}/sandboxes/{id}/fs/rename", broker_base(&state));
    let body = serde_json::json!({ "from": from, "to": to });
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let auth = build_nip98_auth_header(&Method::POST, &url, &bytes, &state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

/// Delete a file or directory inside a sandbox.
#[tauri::command]
pub async fn sandbox_fs_delete(
    sandbox_id: String,
    path: String,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let path = validate_sandbox_path(&path)?;
    let url = format!(
        "{}/sandboxes/{id}/fs?path={}",
        broker_base(&state),
        encode_query_value(path)
    );
    let auth = build_nip98_auth_header(&Method::DELETE, &url, &[], &state)?;
    let response = state
        .http_client
        .delete(&url)
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

// ── Sandbox app launcher ─────────────────────────────────────────────────────
//
// The dock's Browser/Files/Terminal icons don't switch to a flat panel view
// anymore — they open a real window on the sandbox's own desktop, the same
// way double-clicking an app icon would. Computer has no launcher: it's just
// "look at the desktop," which is what the always-visible stage already is.

const LAUNCHABLE_APPS: [&str; 3] = ["browser", "files", "terminal"];

fn validate_launch_app(app: &str) -> Result<&str, String> {
    let app = app.trim();
    if !LAUNCHABLE_APPS.contains(&app) {
        return Err(format!(
            "app must be one of: {}",
            LAUNCHABLE_APPS.join(", ")
        ));
    }
    Ok(app)
}

/// Open a new window of `app` on the sandbox's live desktop.
#[tauri::command]
pub async fn sandbox_launch_app(
    sandbox_id: String,
    app: String,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let app = validate_launch_app(&app)?;
    let url = format!("{}/sandboxes/{id}/launch", broker_base(&state));
    let body = serde_json::json!({ "app": app });
    let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let auth = build_nip98_auth_header(&Method::POST, &url, &bytes, &state)?;
    let response = state
        .http_client
        .post(&url)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(broker_error(status.as_u16(), &text));
    }
    Ok(())
}

// ── Pop-out native window ───────────────────────────────────────────────────
//
// Watching a sandbox's live screen from the sidebar or the agent-profile
// dialog is fine while chatting in the same window, but a second monitor
// needs a real OS window to drag over there. Mirrors `open_huddle_window`:
// dedup by label so re-clicking pop-out while the window is already open
// just brings it forward instead of building a duplicate.

/// Prefix on every computer pop-out window's label, shared with `lib.rs`'s
/// `RunEvent::WindowEvent { CloseRequested, .. }` handling so it can recognize
/// the window and recover the sandbox id from `label.strip_prefix(..)`.
pub(crate) const COMPUTER_WINDOW_LABEL_PREFIX: &str = "computer-";

fn computer_window_label(sandbox_id: &str) -> String {
    format!("{COMPUTER_WINDOW_LABEL_PREFIX}{sandbox_id}")
}

/// Build the `#/computer/<id>?...` hash route for the pop-out window,
/// carrying the parent's already-known sandbox info as query params so the
/// second webview can render immediately instead of waiting on its own
/// community/relay bootstrap to repopulate the shared sandbox store (which
/// left the window stuck on "Connecting…" forever — see
/// `ComputerWindowScreen`'s param-first read). Every param is optional; the
/// frontend falls back to the store for whatever is missing.
fn computer_window_hash_route(id: &str, params: &ComputerWindowParams) -> String {
    let mut query = Vec::new();
    if let Some(viewer_url) = params.viewer_url.as_deref().filter(|s| !s.is_empty()) {
        query.push(format!("viewerUrl={}", encode_query_value(viewer_url)));
    }
    if let Some(sandbox_name) = params.sandbox_name.as_deref().filter(|s| !s.is_empty()) {
        query.push(format!("sandboxName={}", encode_query_value(sandbox_name)));
    }
    if let Some(owner_pubkey) = params.owner_pubkey.as_deref().filter(|s| !s.is_empty()) {
        query.push(format!("ownerPubkey={}", encode_query_value(owner_pubkey)));
    }
    if let Some(agent_display_name) = params
        .agent_display_name
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        query.push(format!(
            "agentDisplayName={}",
            encode_query_value(agent_display_name)
        ));
    }
    if let Some(expires_at) = params.expires_at {
        query.push(format!("expiresAt={expires_at}"));
    }

    if query.is_empty() {
        format!("index.html#/computer/{id}")
    } else {
        format!("index.html#/computer/{id}?{}", query.join("&"))
    }
}

/// Optional sandbox info the parent window already has, passed through to the
/// pop-out so it can render without waiting on its own relay subscription.
/// Mirrors the fields `ComputerPreviewPanel`/`SandboxViewerDialog` already
/// hold from the shared sandbox store.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerWindowParams {
    pub viewer_url: Option<String>,
    pub sandbox_name: Option<String>,
    pub owner_pubkey: Option<String>,
    pub agent_display_name: Option<String>,
    pub expires_at: Option<i64>,
}

/// Open (or focus, if already open) a native window showing `sandbox_id`'s
/// live desktop. The window loads the ordinary app bundle and lands on the
/// `#/computer/<sandboxId>` route, carrying `params` as query params so
/// `ComputerWindowScreen` can mint its own viewer token immediately —
/// this command only owns window lifecycle and the hand-off payload, not the
/// sandbox lookup.
#[tauri::command]
pub async fn open_computer_window(
    sandbox_id: String,
    title: String,
    params: Option<ComputerWindowParams>,
    app: AppHandle,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let label = computer_window_label(id);

    if let Some(window) = app.get_webview_window(&label) {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let window_title = if title.trim().is_empty() {
        "Agent's computer".to_string()
    } else {
        title
    };
    let params = params.unwrap_or_default();

    WebviewWindowBuilder::new(
        &app,
        label,
        WebviewUrl::App(computer_window_hash_route(id, &params).into()),
    )
    .title(window_title)
    .inner_size(1280.0, 820.0)
    .min_inner_size(800.0, 560.0)
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "sandbox_viewer_tests.rs"]
mod tests;
