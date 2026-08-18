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
use crate::relay::build_nip98_auth_header;

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
    let body = serde_json::json!({
        "image": SELF_SERVICE_IMAGE,
        "owner": agent_pubkey,
        "ttl_seconds": SELF_SERVICE_TTL_SECONDS,
        "env": {
            "BUZZ_DEV_MCP_BIND": "0.0.0.0:9320",
            "BUZZ_DEV_MCP_OWNER": agent_pubkey,
            "BUZZ_DESKTOP_ENABLED": "1",
        },
    });
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
        attach_sandbox_to_local_agent(&app, &agent_pubkey, sandbox_id)?;
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
fn attach_sandbox_to_local_agent(
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
    ) {
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

    detach_sandbox_from_local_agent(&app, id);
    Ok(())
}

/// Find whichever locally-managed agent has `sandbox_id` attached, clear it,
/// and restart that agent. A no-op (not an error, and not surfaced to the
/// caller) when no local record has this sandbox attached — the box may
/// belong to an agent unmanaged on this machine, or to no agent at all.
fn detach_sandbox_from_local_agent(app: &AppHandle, sandbox_id: &str) {
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
            ) {
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
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("could not read the downloaded file: {e}"))?;

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
    std::fs::write(&dest, &bytes).map_err(|e| format!("could not save the file: {e}"))?;

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
    let bytes =
        std::fs::read(&local_path).map_err(|e| format!("could not read the local file: {e}"))?;
    if bytes.len() > 50 * 1024 * 1024 {
        return Err("file is larger than the 50MB upload limit".to_string());
    }
    let url = format!(
        "{}/sandboxes/{id}/fs/file?path={}",
        broker_base(&state),
        encode_query_value(dest_path)
    );
    let auth = build_nip98_auth_header(&Method::PUT, &url, &bytes, &state)?;
    let response = state
        .http_client
        .put(&url)
        .header("Authorization", auth)
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

/// List the open windows on the sandbox's desktop (id, title, active) —
/// the dock's taskbar segment polls this.
#[tauri::command]
pub async fn sandbox_list_windows(
    sandbox_id: String,
    state: State<'_, AppState>,
) -> CmdResult<serde_json::Value> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let url = format!("{}/sandboxes/{id}/windows", broker_base(&state));
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

const WINDOW_ACTIONS: &[&str] = &["activate", "minimize", "close"];

fn validate_window_action(action: &str) -> Result<&str, String> {
    let action = action.trim();
    if !WINDOW_ACTIONS.contains(&action) {
        return Err(format!(
            "action must be one of: {}",
            WINDOW_ACTIONS.join(", ")
        ));
    }
    Ok(action)
}

/// A window id as the broker reports it: `0x`-prefixed hex or decimal.
/// Checked here too so a malformed value never even leaves the app.
fn validate_window_id(window: &str) -> Result<&str, String> {
    let window = window.trim();
    let digits = window.strip_prefix("0x").unwrap_or(window);
    let is_hex = window.starts_with("0x");
    let ok = !digits.is_empty()
        && digits.len() <= 16
        && digits.bytes().all(|b| {
            if is_hex {
                b.is_ascii_hexdigit()
            } else {
                b.is_ascii_digit()
            }
        });
    if !ok {
        return Err("malformed window id".to_string());
    }
    Ok(window)
}

/// Focus, minimize, or close one window on the sandbox's desktop — the
/// dock's taskbar clicks.
#[tauri::command]
pub async fn sandbox_window_action(
    sandbox_id: String,
    window: String,
    action: String,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let id = validate_sandbox_id(&sandbox_id)?;
    let window = validate_window_id(&window)?;
    let action = validate_window_action(&action)?;
    let url = format!("{}/sandboxes/{id}/windows/action", broker_base(&state));
    let body = serde_json::json!({ "window": window, "action": action });
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

fn computer_window_label(sandbox_id: &str) -> String {
    format!("computer-{sandbox_id}")
}

/// Open (or focus, if already open) a native window showing `sandbox_id`'s
/// live desktop. The window loads the ordinary app bundle and lands on the
/// `#/computer/<sandboxId>` route, which resolves the sandbox from the
/// shared frontend store and mints its own viewer token — this command only
/// owns window lifecycle, not the sandbox lookup or the viewer URL.
#[tauri::command]
pub async fn open_computer_window(
    sandbox_id: String,
    title: String,
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

    WebviewWindowBuilder::new(
        &app,
        label,
        WebviewUrl::App(format!("index.html#/computer/{id}").into()),
    )
    .title(window_title)
    .inner_size(1280.0, 820.0)
    .min_inner_size(800.0, 560.0)
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{computer_window_label, to_base64url, validate_sandbox_id};
    use base64::Engine;

    #[test]
    fn reencodes_standard_base64_as_url_safe() {
        // A payload whose standard base64 contains + and / must come back with
        // only URL-safe characters (- _), and decode to the same bytes.
        let raw = b"\xfb\xff\xbf hello world >>";
        let std = base64::engine::general_purpose::STANDARD.encode(raw);
        let url = to_base64url(&std).unwrap();
        assert!(!url.contains('+') && !url.contains('/') && !url.contains('='));
        let back = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&url)
            .unwrap();
        assert_eq!(back, raw);
    }

    #[test]
    fn computer_window_label_is_stable_and_unique_per_sandbox() {
        // `open_computer_window` dedups via `app.get_webview_window(&label)`,
        // so two calls for the same sandbox id must produce the exact same
        // label (or a second click would build a duplicate window instead of
        // focusing the existing one), while two different sandboxes must not
        // collide.
        let id = validate_sandbox_id("buzz-sandbox-fq5ijxh7lt").unwrap();
        assert_eq!(
            computer_window_label(id),
            computer_window_label(id),
            "label must be deterministic for the same sandbox id"
        );
        assert_eq!(
            computer_window_label(id),
            "computer-buzz-sandbox-fq5ijxh7lt"
        );
        assert_ne!(
            computer_window_label(id),
            computer_window_label("other-sandbox-id")
        );
    }
}
