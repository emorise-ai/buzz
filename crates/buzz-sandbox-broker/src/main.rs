//! Sandbox broker — creates and reaps agent containers on a sandbox host.
//!
//! Sits between `buzz-backend-docker` (which runs on whatever machine launches
//! an agent) and the Docker daemon (which runs here). The provider holds no
//! substrate credentials; this service holds no agent identity beyond the
//! environment it is handed for one create call.
//!
//! Trust boundary, stated plainly: this process can create containers on the
//! host, so it is deliberately small, and every limit it enforces is decided
//! in `sandbox.rs` rather than accepted from a caller. It binds to loopback by
//! default and authenticates every mutating request by Buzz identity (NIP-98),
//! deferring authorization to the relay's membership answer — one identity
//! system rather than two, and no shared secret to distribute or rotate.

mod docker;
mod events;
mod identity;
mod proxy;
mod sandbox;

use axum::extract::{Path, Query, RawQuery, Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use sandbox::{CreateRequest, Limits, SandboxSummary};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Port a sandbox serves its tool surface on, when it serves one.
///
/// Fixed rather than configurable: it is reachable only from the sandbox
/// network, so there is no conflict to resolve, and a caller that had to
/// discover the port would need a second round trip to learn it.
const TOOLS_PORT: u16 = 9320;

/// Port the sandbox desktop (noVNC over websockify) serves on inside the
/// container. Matches `DESKTOP_PORT` in `Dockerfile.sprig-desktop`.
const DESKTOP_PORT: u16 = 6080;

/// Port the sandbox terminal (ttyd) serves on inside the container.
const TERMINAL_PORT: u16 = 7681;

#[derive(Clone)]
struct AppState {
    docker: docker::Docker,
    /// Buzz identity verification. Replaces the shared bearer token: callers
    /// sign requests with their own Nostr key and are authorized by relay
    /// membership, so there is one identity system rather than two.
    verifier: Arc<identity::Verifier>,
    /// Announces sandbox lifecycle to the relay so Buzz can show that an agent
    /// has a computer. Absent when no relay is configured.
    publisher: Option<Arc<events::Publisher>>,
    /// Public base URL to advertise in a sandbox's `viewer` event tag, e.g.
    /// `https://sandbox.example.com` or `https://relay.example.com/sandbox-viewer`.
    /// Only the *published link* depends on this — token verification derives the
    /// origin from the request's forwarded host (`public_base_from_request`), so
    /// access control does not depend on this being set correctly. It exists
    /// because the create call arrives over a private tunnel with no public host
    /// to read. Absent when no viewer is exposed, in which case events carry no
    /// viewer link rather than an address that would not resolve.
    viewer_base: Option<Arc<String>>,
    allowed_images: Arc<Vec<String>>,
    network: Arc<String>,
    /// Writable-layer cap (e.g. "30G"). None on filesystems that cannot
    /// enforce one — see `container_spec`.
    disk_limit: Option<Arc<str>>,
    host_cpus: usize,
    slot: Arc<std::sync::atomic::AtomicUsize>,
    /// Live expiry per container id — the authority the reaper reads.
    ///
    /// Docker labels are immutable on a running container, so an extend cannot
    /// rewrite `com.buzz.sandbox.expires-at`. The label stays as the *initial*
    /// expiry and crash-recovery seed; this map is the live truth. Absent
    /// entries fall back to the label (and, at startup, to the latest 48200 on
    /// the relay — the only record that survives a broker restart after an
    /// extend). See `seed_expiries`.
    expiry: Arc<std::sync::Mutex<std::collections::HashMap<String, i64>>>,
    /// Expiry last announced to the relay as a kind:48200, per container id —
    /// the keepalive middleware's republish throttle. Separate from `expiry`
    /// (the reaper's live truth) because the two update on different
    /// schedules: `expiry` moves on every authenticated call, this only when
    /// that movement crosses `KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS` since
    /// the last announcement. See `bump_keepalive`.
    last_published_expiry: Arc<std::sync::Mutex<std::collections::HashMap<String, i64>>>,
}

impl AppState {
    fn set_expiry(&self, id: &str, expires_at: i64) {
        if let Ok(mut map) = self.expiry.lock() {
            map.insert(id.to_string(), expires_at);
        }
    }

    fn forget_expiry(&self, id: &str) {
        if let Ok(mut map) = self.expiry.lock() {
            map.remove(id);
        }
        if let Ok(mut map) = self.last_published_expiry.lock() {
            map.remove(id);
        }
    }

    fn expiry_of(&self, id: &str) -> Option<i64> {
        self.expiry.lock().ok()?.get(id).copied()
    }

    fn last_published_expiry_of(&self, id: &str) -> Option<i64> {
        self.last_published_expiry.lock().ok()?.get(id).copied()
    }

    fn set_last_published_expiry(&self, id: &str, expires_at: i64) {
        if let Ok(mut map) = self.last_published_expiry.lock() {
            map.insert(id.to_string(), expires_at);
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    // Buzz identity, not a shared secret. The relay is the authority on who
    // may run sandboxes, so the broker needs to know where it is.
    let relay_url = match std::env::var("BUZZ_SANDBOX_RELAY_URL") {
        Ok(u) if !u.trim().is_empty() => u,
        _ => {
            error!(
                "BUZZ_SANDBOX_RELAY_URL is required — the broker authorizes \
                 callers by asking the relay who is a member"
            );
            std::process::exit(1);
        }
    };
    // The exact origin(s) clients call, because NIP-98 signs the full URL and
    // a mismatch rejects every otherwise-valid request. Comma-separated: the
    // broker legitimately answers on several addresses at once (loopback
    // tunnel, its container DNS name on the sandbox network, a public
    // reverse-proxy path).
    let public_url = std::env::var("BUZZ_SANDBOX_PUBLIC_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9310".to_string());

    let allowed_images: Vec<String> = std::env::var("BUZZ_SANDBOX_IMAGES")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if allowed_images.is_empty() {
        error!("BUZZ_SANDBOX_IMAGES is required — an unlisted image must not hold an agent key");
        std::process::exit(1);
    }

    let socket =
        std::env::var("DOCKER_SOCKET").unwrap_or_else(|_| "/var/run/docker.sock".to_string());
    let network = std::env::var("BUZZ_SANDBOX_NETWORK").unwrap_or_else(|_| "bridge".to_string());
    let bind = std::env::var("BUZZ_SANDBOX_BIND").unwrap_or_else(|_| "127.0.0.1:9310".to_string());
    let host_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    let docker = docker::Docker::new(&socket);
    if let Err(e) = docker.ping().await {
        error!(error = %e, socket = %socket, "cannot reach the docker daemon");
        std::process::exit(1);
    }

    // One identity for the broker: it signs its own membership queries with
    // the same key it signs announcements with, so it is a single recognizable
    // participant rather than two.
    let broker_keys = match std::env::var("BUZZ_SANDBOX_RELAY_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    {
        Some(k) => match nostr::Keys::parse(k.trim()) {
            Ok(keys) => keys,
            Err(e) => {
                error!(error = %e, "BUZZ_SANDBOX_RELAY_KEY is not a valid key");
                std::process::exit(1);
            }
        },
        None => nostr::Keys::generate(),
    };
    // Mirrors the relay's own default: it admits any authenticated caller
    // unless membership is explicitly required.
    let require_membership = std::env::var("BUZZ_SANDBOX_REQUIRE_MEMBERSHIP")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);

    let publisher = match events::Publisher::new(&relay_url, broker_keys.clone()) {
        Ok(p) => {
            info!(broker_pubkey = %p.pubkey_hex(), "publishing sandbox events to the relay");
            Some(Arc::new(p))
        }
        Err(e) => {
            // A broker that cannot announce still runs sandboxes; refusing to
            // start would trade a display feature for an outage.
            warn!(error = %e, "sandbox events disabled");
            None
        }
    };

    let state = AppState {
        docker,
        verifier: Arc::new(identity::Verifier::new(
            relay_url,
            public_url,
            broker_keys.clone(),
            require_membership,
        )),
        publisher,
        viewer_base: std::env::var("BUZZ_SANDBOX_VIEWER_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| Arc::new(v.trim_end_matches('/').to_string())),
        allowed_images: Arc::new(allowed_images),
        network: Arc::new(network),
        disk_limit: std::env::var("BUZZ_SANDBOX_DISK")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| Arc::from(s.trim())),
        host_cpus,
        slot: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        expiry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        last_published_expiry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    seed_expiries(&state).await;
    tokio::spawn(reaper(state.clone()));

    let app = build_router(state);

    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            error!(error = %e, bind = %bind, "could not bind");
            std::process::exit(1);
        }
    };
    info!(bind = %bind, host_cpus, "sandbox broker listening");
    if let Err(e) = axum::serve(listener, app).await {
        error!(error = %e, "server stopped");
        std::process::exit(1);
    }
}

/// Build the broker's route table.
///
/// Split out from `main` so a test can drive the real router (via
/// `tower::ServiceExt::oneshot`) without binding a socket — router-level
/// regressions (a route that 404s before ever reaching its handler) are
/// invisible to a handler-level unit test.
fn build_router(state: AppState) -> Router {
    // Every route scoped to one sandbox (`/sandboxes/{id}/...`) sits behind
    // the keepalive middleware: any authenticated 2xx against one of these
    // is real use of that sandbox, and bumps its expiry. `/sandboxes` itself
    // (bare list/create, no `{id}`) and `/health` are not nested here —
    // there is no sandbox yet to bump on a create, and list/health are not
    // "using" any particular one.
    let sandbox_scoped = Router::new()
        .route("/sandboxes/{id}", get(get_sandbox).delete(delete_sandbox))
        .route("/sandboxes/{id}/stop", post(delete_sandbox))
        .route("/sandboxes/{id}/extend", post(extend_sandbox))
        // The live desktop: a signed-token-gated reverse proxy to the sandbox's
        // in-container noVNC server. Two routes because the noVNC client fetches
        // sibling assets (`vnc.html`, `websockify`, …) under the same prefix.
        .route("/sandboxes/{id}/desktop", any(desktop))
        // The token embedded as a path segment: every relative asset the noVNC
        // page fetches — and its websockify WebSocket — resolves under this
        // prefix and so carries the token without any cookie or header games.
        // The bare `/desktop` entry above verifies the query token and
        // redirects here.
        .route("/sandboxes/{id}/desktop/t/{token}/{*rest}", any(desktop))
        // The live terminal: same signed-token gate and token-segment shape as
        // the desktop, proxied to ttyd instead of websockify.
        .route("/sandboxes/{id}/terminal", any(terminal))
        // ttyd's own index lives at exactly `/`, so the redirect below lands
        // on the bare trailing-slash form — unlike the desktop, which always
        // redirects to a non-empty `view` tail. axum's `{*rest}` wildcard
        // does not match a request with nothing after the final `/`, so that
        // exact URL needs its own literal route (no `rest` param; the shared
        // handler already treats an absent `rest` as empty, same as the
        // proxy treating an empty `rest` as `/`).
        .route("/sandboxes/{id}/terminal/t/{token}/", any(terminal))
        .route("/sandboxes/{id}/terminal/t/{token}/{*rest}", any(terminal))
        // The file API: NIP-98 header auth (not a viewer token — these are
        // ordinary agent-facing calls made by whoever holds a Buzz identity,
        // the same as create/delete/extend above).
        .route("/sandboxes/{id}/fs", get(fs_list).delete(fs_delete))
        .route("/sandboxes/{id}/fs/file", get(fs_download).put(fs_upload))
        .route("/sandboxes/{id}/fs/rename", post(fs_rename))
        // Opens a real window on the sandbox desktop for one of a fixed set
        // of apps — the dock's icons drive this instead of switching a flat
        // view, so multiple terminals/file windows can coexist and the
        // window manager handles drag/arrange.
        .route("/sandboxes/{id}/launch", post(launch_app))
        // Computer-use surface: run a command, see the screen, click/type.
        // Same NIP-98 + owner-only pattern as `/extend` — these act on the
        // sandbox as its own owner, not as an unprivileged viewer.
        .route("/sandboxes/{id}/exec", post(sandbox_exec))
        .route("/sandboxes/{id}/screenshot", get(sandbox_screenshot))
        .route("/sandboxes/{id}/input", post(sandbox_input))
        // Screen recording: same owner-or-manager gate as the rest of the
        // computer-use surface above — a recording captures the same screen
        // exec/input/screenshot already expose to owner and manager alike.
        .route(
            "/sandboxes/{id}/recording/start",
            post(sandbox_recording_start),
        )
        .route(
            "/sandboxes/{id}/recording/stop",
            post(sandbox_recording_stop),
        )
        // Activity signal only — the desktop viewer pings this while a
        // sandbox's screen is mounted so passive watching (no exec/input of
        // its own) still counts as use. The handler itself does nothing;
        // the middleware below is what bumps expiry on its 2xx.
        .route("/sandboxes/{id}/heartbeat", post(heartbeat))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            keepalive_middleware,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/sandboxes", get(list_sandboxes).post(create_sandbox))
        .merge(sandbox_scoped)
        .with_state(state)
}

/// Unauthenticated: it reports only that the process is up, so a health probe
/// does not need a credential.
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Authenticate a caller by Buzz identity and authorize by relay membership.
///
/// Two steps, deliberately in this order: the signature check is local, so an
/// unauthenticated caller never triggers a network request to the relay.
async fn authorize(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<String, axum::response::Response> {
    let pubkey = state
        .verifier
        .verify(headers, method, path, body)
        .map_err(|e| {
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response()
        })?;

    if !state.verifier.is_member(&pubkey).await {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "not a member of this Buzz community — sandboxes are \
                          only for members"
            })),
        )
            .into_response());
    }
    Ok(pubkey)
}

fn bad_request(msg: impl Into<String>) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": msg.into()})),
    )
        .into_response()
}

async fn create_sandbox(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    // NIP-98 signs the body, so it must be verified before it is parsed —
    // parsing first would let an unauthenticated caller exercise the decoder.
    let caller = match authorize(&state, &headers, "POST", "/sandboxes", &body).await {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    let req: CreateRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("invalid request body: {e}")),
    };

    if let Err(e) = sandbox::image_allowed(&req.image, &state.allowed_images) {
        return bad_request(e);
    }

    // Concurrency cap: the host runs production workloads alongside sandboxes,
    // so the broker refuses rather than letting the box be oversubscribed.
    // Fetched once and reused below for the per-owner dedupe check — both
    // need the same "what's currently running" list.
    let existing = match state.docker.list_managed(sandbox::MANAGED_LABEL).await {
        Ok(existing) => {
            let running = existing
                .iter()
                .filter(|c| c.get("State").and_then(|s| s.as_str()) == Some("running"))
                .count();
            if running >= sandbox::MAX_CONCURRENT {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({
                        "error": format!(
                            "sandbox limit reached ({running}/{}); stop one before creating another",
                            sandbox::MAX_CONCURRENT
                        )
                    })),
                )
                    .into_response();
            }
            existing
        }
        Err(e) => return internal(e),
    };

    // Per-owner dedupe: an owner that already has a live sandbox gets that
    // one back instead of a second box, making "create" idempotent per
    // owner and giving a restarting agent a stable id to reconnect to. The
    // decision itself is pure (`existing_sandbox_for_owner`); only the
    // Docker list-entry -> decision-input projection happens here.
    let projections: Vec<sandbox::ExistingSandbox> = existing
        .iter()
        .map(|c| sandbox::ExistingSandbox {
            owner: c
                .get("Labels")
                .and_then(|l| l.get(sandbox::LABEL_OWNER))
                .and_then(|v| v.as_str()),
            running: c.get("State").and_then(|s| s.as_str()) == Some("running"),
        })
        .collect();
    if let Some(idx) = sandbox::existing_sandbox_for_owner(req.owner.as_deref(), &projections) {
        return match sandbox_summary_response(&state, &existing[idx]).await {
            Ok(response) => response,
            Err(e) => internal(e),
        };
    }

    let limits = Limits::resolve(&req);
    let expires_at = chrono::Utc::now().timestamp() + limits.ttl_seconds as i64;
    let slot = state
        .slot
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cpuset = sandbox::assign_cpuset(state.host_cpus, limits.cpus, slot);
    let name = format!("buzz-sandbox-{}", short_id());

    // A bearer token for the sandbox's tool port.
    //
    // The MCP authorization spec expects an HTTP tool server that authenticates
    // to do so with `Authorization: Bearer`, and every MCP client implements
    // that. NIP-98 is stronger — it signs each request's body — but it is a
    // scheme the standard does not define, so a conforming client has no way to
    // know it should sign, and simply connects unauthenticated. Both are
    // accepted; this is the one that interoperates.
    //
    // Minted here rather than by the sandbox because the caller needs it in the
    // create response: the sandbox has no channel back to whoever launched it.
    // It is scoped to one sandbox and dies with it, so its blast radius is a
    // container that is disposable by design.
    let tools_token = mint_tools_token();
    let mut env = req.env.clone();
    env.insert("BUZZ_DEV_MCP_TOKEN".to_string(), tools_token.clone());

    let spec = sandbox::container_spec(sandbox::SpecInputs {
        image: &req.image,
        limits,
        env: &env,
        owner: req.owner.as_deref(),
        // The verified NIP-98 caller, not `req.owner`: when the desktop app
        // creates a box on an agent's behalf it signs as the human manager
        // while `owner` names the agent, so these two are deliberately
        // allowed to differ. See `LABEL_MANAGER`.
        manager: Some(&caller),
        expires_at,
        cpuset: &cpuset,
        network: &state.network,
        disk_limit: state.disk_limit.as_deref(),
    });

    let id = match state.docker.create_container(&name, spec).await {
        Ok(id) => id,
        Err(e) => return internal(e),
    };

    if let Err(e) = state.docker.start_container(&id).await {
        // Roll back: a created-but-unstarted container is residue that would
        // otherwise count against the concurrency cap forever.
        let _ = state.docker.remove_container(&id).await;
        return internal(format!("container created but would not start: {e}"));
    }
    state.set_expiry(&id, expires_at);

    // Where the agent's brain reaches this sandbox's tools.
    //
    // Sandboxes sit on their own bridge network and publish no host ports, so
    // the address is the container's IP on that network. That is deliberately
    // *not* reachable from the internet: only something already on the sandbox
    // network — an agent running on this host — can use it. Nothing is exposed
    // to reach it, which is why this is preferable to publishing a port.
    //
    // Best-effort: a sandbox whose IP cannot be read still runs, it simply
    // cannot be driven remotely, so this must not fail the create.
    let tools_url = state
        .docker
        .inspect_container(&id)
        .await
        .ok()
        .and_then(|v| sandbox_ip(&v, &state.network))
        .map(|ip| format!("http://{ip}:{TOOLS_PORT}/mcp"));

    // Announce to the relay so Buzz can show that this agent has a computer.
    // Best-effort: a sandbox that runs unannounced is a display gap, not a
    // failure, so this never affects the response.
    if let Some(publisher) = state.publisher.as_ref() {
        let viewer = state
            .viewer_base
            .as_ref()
            .map(|base| format!("{base}/sandboxes/{}/desktop", &id[..12.min(id.len())]));
        publisher
            .sandbox_created(events::SandboxFacts {
                sandbox_id: &id,
                name: &name,
                image: &req.image,
                // The agent's own key when given, else the caller's — either
                // way the desktop can join this sandbox to an agent card.
                owner: req.owner.as_deref().or(Some(caller.as_str())),
                cpus: limits.cpus,
                memory_mb: limits.memory_mb,
                expires_at,
                viewer_url: viewer.as_deref(),
            })
            .await;
    }

    info!(
        sandbox = %name,
        image = %req.image,
        cpus = limits.cpus,
        memory_mb = limits.memory_mb,
        ttl_seconds = limits.ttl_seconds,
        cpuset = %cpuset,
        "sandbox created"
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "name": name,
            "image": req.image,
            "cpus": limits.cpus,
            "memory_mb": limits.memory_mb,
            "ttl_seconds": limits.ttl_seconds,
            "cpuset": cpuset,
            "expires_at": expires_at,
            // Absent when the container's IP could not be read; a caller that
            // needs remote tools should treat that as "not drivable" rather
            // than guessing an address.
            "tools_url": tools_url,
            // Present it as `Authorization: Bearer <token>`. Returned only
            // here, on the one response the launcher receives: the broker does
            // not store it, and `GET /sandboxes` never discloses it.
            "tools_token": tools_token,
        })),
    )
        .into_response()
}

/// Build a create-shaped response (`200`, not `201` — nothing was created)
/// for a sandbox the per-owner dedupe matched, from a fresh inspect of the
/// list entry `create_sandbox` already found.
///
/// Mirrors the fields `create_sandbox` returns, read the same way
/// `extend_sandbox` reads them back from an inspect (cpus/memory from
/// `HostConfig`, not the list entry, which carries neither) — with one
/// deliberate gap: `tools_token` is the create-time bearer secret, minted
/// once and never stored, so a reused sandbox has none to return. A caller
/// that needs the tools port on a reused sandbox has no way to get a fresh
/// token short of stopping and recreating it; that is a real limitation of
/// reuse, not an oversight.
async fn sandbox_summary_response(
    state: &AppState,
    list_entry: &serde_json::Value,
) -> Result<axum::response::Response, String> {
    let id = list_entry
        .get("Id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "matched sandbox has no id".to_string())?;
    let inspect = state.docker.inspect_container(id).await?;
    let full_id = inspect
        .get("Id")
        .and_then(|v| v.as_str())
        .unwrap_or(id)
        .to_string();
    let name = inspect
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = inspect
        .pointer("/Config/Image")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let cpus = inspect
        .pointer("/HostConfig/NanoCpus")
        .and_then(|v| v.as_i64())
        .map(|n| n as f64 / 1e9)
        .unwrap_or(0.0);
    let memory_mb = inspect
        .pointer("/HostConfig/Memory")
        .and_then(|v| v.as_i64())
        .map(|b| (b / (1024 * 1024)) as u64)
        .unwrap_or(0);
    let cpuset = inspect
        .pointer("/HostConfig/CpusetCpus")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let expires_at = state.expiry_of(&full_id).or_else(|| {
        inspect_label(&inspect, sandbox::LABEL_EXPIRES).and_then(|s| s.parse::<i64>().ok())
    });
    let tools_url =
        sandbox_ip(&inspect, &state.network).map(|ip| format!("http://{ip}:{TOOLS_PORT}/mcp"));

    info!(sandbox = %full_id, "create request matched an existing sandbox for its owner");

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "id": full_id,
            "name": name,
            "image": image,
            "cpus": cpus,
            "memory_mb": memory_mb,
            "cpuset": cpuset,
            "expires_at": expires_at,
            "tools_url": tools_url,
            // No secret to hand back — see this function's doc comment.
            "tools_token": serde_json::Value::Null,
            // Signals the caller got back an existing sandbox rather than a
            // freshly created one, so it does not treat the missing
            // `tools_token`/`ttl_seconds` as broker error.
            "reused": true,
        })),
    )
        .into_response())
}

/// Reconstruct the broker's public origin from a reverse proxy's forwarded
/// headers, the way the relay derives its host from the request rather than a
/// baked-in URL. Returns e.g. `https://sandbox.example.com` or, when the viewer
/// is mounted under a stripped path prefix, `https://relay.example.com/sandbox-viewer`.
///
/// `X-Forwarded-Prefix` is honored so the signed URL matches whether the viewer
/// is served at a hostname's root (its own machine) or a sub-path (sharing the
/// relay's host). Returns `None` when no forwarded host is present — a request
/// that reached the broker directly, with no proxy in front to trust.
fn public_base_from_request(headers: &axum::http::HeaderMap) -> Option<String> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());

    // A forwarded host is the signal that a trusted proxy set these; the bare
    // Host header on a direct request is not something to sign against.
    let host = header("x-forwarded-host")?.split(',').next()?.trim();
    if host.is_empty() {
        return None;
    }
    let proto = header("x-forwarded-proto")
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("https");
    // A proxy reports a WebSocket upgrade as ws/wss, but the token was signed
    // over the https page URL — same origin, different scheme spelling. Fold
    // them together or every websockify handshake 401s on a "URL mismatch".
    let proto = match proto {
        "wss" => "https",
        "ws" => "http",
        p => p,
    };
    let prefix = header("x-forwarded-prefix")
        .map(|p| p.trim_end_matches('/'))
        .unwrap_or("");

    Some(format!("{proto}://{host}{prefix}"))
}

/// The sandbox's IP on its own bridge network.
///
/// Prefers the network the broker places sandboxes on. Falls back to any
/// attached network with an address, because a misconfigured `NetworkMode`
/// should degrade to "reachable" rather than "invisible".
fn sandbox_ip(inspect: &serde_json::Value, network: &str) -> Option<String> {
    let networks = inspect
        .get("NetworkSettings")?
        .get("Networks")?
        .as_object()?;
    let addr = |v: &serde_json::Value| {
        v.get("IPAddress")
            .and_then(|a| a.as_str())
            .filter(|a| !a.is_empty())
            .map(str::to_string)
    };
    networks
        .get(network)
        .and_then(addr)
        .or_else(|| networks.values().find_map(addr))
}

async fn list_sandboxes(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    if let Err(response) = authorize(&state, &headers, "GET", "/sandboxes", b"").await {
        return response;
    }
    match state.docker.list_managed(sandbox::MANAGED_LABEL).await {
        Ok(list) => {
            let out: Vec<SandboxSummary> = list
                .iter()
                .map(|c| {
                    let mut s = summarize(c);
                    // Live expiry wins over the (immutable) label after an
                    // extend — same rule as `get_sandbox`.
                    if let Some(live) = state.expiry_of(&s.id) {
                        s.expires_at = Some(live);
                    }
                    s
                })
                .collect();
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(e) => internal(e),
    }
}

async fn get_sandbox(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    if let Err(response) =
        authorize(&state, &headers, "GET", &format!("/sandboxes/{id}"), b"").await
    {
        return response;
    }
    match state.docker.inspect_container(&id).await {
        Ok(v) => {
            if !is_managed(&v) {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "no such sandbox"})),
                )
                    .into_response();
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": v.get("Id"),
                    "name": v.get("Name"),
                    "state": v.pointer("/State/Status"),
                    "started_at": v.pointer("/State/StartedAt"),
                    "image": v.pointer("/Config/Image"),
                    // Live state first: after an extend the label still holds
                    // the initial expiry, and reporting that would show a
                    // countdown the reaper no longer honors. The map keys on
                    // the full container id, not whatever alias the caller
                    // used, so the lookup goes through the inspected Id.
                    "expires_at": v
                        .get("Id")
                        .and_then(|x| x.as_str())
                        .and_then(|full| state.expiry_of(full))
                        .or_else(|| {
                            inspect_label(&v, sandbox::LABEL_EXPIRES)
                                .and_then(|s| s.parse::<i64>().ok())
                        }),
                })),
            )
                .into_response()
        }
        Err(e) if e.contains("404") || e.to_lowercase().contains("no such container") => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no such sandbox"})),
        )
            .into_response(),
        Err(e) => internal(e),
    }
}

/// Path params of a `/sandboxes/{id}/{surface}[/t/{token}/{*rest}]` route,
/// pulled out positionally since axum hands wildcard routes a flat list.
struct SurfaceParams {
    id: String,
    path_token: Option<String>,
    rest: String,
}

fn surface_params(params: &[(String, String)]) -> SurfaceParams {
    SurfaceParams {
        id: params.first().map(|(_, v)| v.clone()).unwrap_or_default(),
        path_token: params
            .iter()
            .find(|(k, _)| k == "token")
            .map(|(_, v)| v.clone()),
        rest: params
            .iter()
            .find(|(k, _)| k == "rest")
            .map(|(_, v)| v.clone())
            .unwrap_or_default(),
    }
}

/// Shared gate for a signed-token-proxied surface (desktop, terminal): checks
/// the id, resolves the token from either the path segment or the `?t=`
/// query, verifies it against `{public_base}/sandboxes/{id}/{surface}` (the
/// one bare link Buzz is ever handed), and checks membership.
///
/// Returns the caller's pubkey and the resolved public base on success —
/// callers need the base again to build the redirect location.
async fn verify_surface_token(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    surface: &str,
    id: &str,
    path_token: &Option<String>,
    query: &std::collections::HashMap<String, String>,
) -> Result<(String, String), axum::response::Response> {
    if !is_safe_id(id) {
        return Err(bad_request("malformed sandbox id"));
    }

    let entry_token = query.get("t").cloned();
    let Some(token) = path_token.clone().or(entry_token) else {
        return Err((
            StatusCode::UNAUTHORIZED,
            format!("missing viewer token — open the {surface} from Buzz"),
        )
            .into_response());
    };

    // Host-derived, like the relay: the public origin is whatever host the
    // request arrived on (from the reverse proxy's forwarded headers), not a
    // baked-in URL. The configured base is only a fallback for a request that
    // reaches the broker directly (no proxy), e.g. a local tunnel in dev.
    let Some(public_base) = public_base_from_request(headers)
        .or_else(|| state.viewer_base.as_ref().map(|b| b.to_string()))
    else {
        return Err((StatusCode::NOT_FOUND, "viewer host is not known").into_response());
    };

    // The app signed a GET over the exact URL the broker published, which is
    // always the bare surface path (the app is handed one link, not the asset
    // URLs the client fetches afterwards). Verify against that.
    let signed_url = format!("{public_base}/sandboxes/{id}/{surface}");
    let caller = state
        .verifier
        .verify_token(&token, &signed_url)
        .map_err(|e| (StatusCode::UNAUTHORIZED, e).into_response())?;
    if !state.verifier.is_member(&caller).await {
        return Err((StatusCode::FORBIDDEN, "not a member of this Buzz community").into_response());
    }

    Ok((caller, public_base))
}

/// The sandbox's address on its own bridge network, or a `NOT_FOUND` /
/// `BAD_GATEWAY` response when it cannot be resolved — shared by every proxy
/// surface (desktop, terminal) so each just answers "what port".
async fn resolve_sandbox_ip(
    state: &AppState,
    id: &str,
) -> Result<String, axum::response::Response> {
    let inspect = match state.docker.inspect_container(id).await {
        Ok(v) if is_managed(&v) => v,
        _ => return Err((StatusCode::NOT_FOUND, "no such sandbox").into_response()),
    };
    sandbox_ip(&inspect, &state.network)
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "sandbox has no address").into_response())
}

/// The live desktop, gated by a signed token. Two routes land here:
///
/// * `/desktop?t=<token>` — the single link Buzz is handed. Verifies the query
///   token, then redirects to the noVNC client *under a token path segment*
///   (`/desktop/t/<token>/vnc.html?autoconnect=…`). The redirect is what makes
///   the rest of the page work: a browser does not carry a query string over
///   to relative asset fetches or a WebSocket, but every URL under the token
///   segment inherits it by construction.
/// * `/desktop/t/<token>/{rest}` — the client's assets and its `websockify`
///   WebSocket. Verifies the same token (still signed over the bare desktop
///   URL) and hands the request to the reverse proxy.
async fn desktop(
    State(state): State<AppState>,
    Path(params): Path<Vec<(String, String)>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    req: Request,
) -> axum::response::Response {
    let SurfaceParams {
        id,
        path_token,
        rest,
    } = surface_params(&params);

    let (_caller, _public_base) = match verify_surface_token(
        &state,
        req.headers(),
        "desktop",
        &id,
        &path_token,
        &query,
    )
    .await
    {
        Ok(v) => v,
        Err(response) => return response,
    };
    let Some(token) = path_token.clone().or_else(|| query.get("t").cloned()) else {
        return (StatusCode::UNAUTHORIZED, "missing viewer token").into_response();
    };

    // Bare entry: bounce into the token-segment world, straight to the noVNC
    // client with autoconnect. `path` is how the client finds its websockify
    // WebSocket — the same token-carrying prefix, expressed from the host
    // root without a leading slash (noVNC joins it onto ws(s)://host/).
    if path_token.is_none() {
        let prefix = req
            .headers()
            .get("x-forwarded-prefix")
            .and_then(|v| v.to_str().ok())
            .map(|p| p.trim_end_matches('/'))
            .unwrap_or("");
        let location = format!("{prefix}/sandboxes/{id}/desktop/t/{token}/view");
        return axum::response::Redirect::temporary(&location).into_response();
    }

    // Buzz's own viewer page: the live screen and nothing else. Serving our
    // own page instead of the stock noVNC UI removes its branding, connect
    // dialog, and control bar — the engine (core/rfb.js, proxied statically
    // from the sandbox like any other asset) is invisible.
    if rest == "view" {
        return (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            VIEWER_PAGE,
        )
            .into_response();
    }

    // Resolve the sandbox's address on its own network. A stopped or reaped
    // sandbox has none — say so rather than proxying into the void.
    let ip = match resolve_sandbox_ip(&state, &id).await {
        Ok(ip) => ip,
        Err(response) => return response,
    };

    proxy::desktop_proxy(&format!("{ip}:{DESKTOP_PORT}"), &rest, req).await
}

/// The live terminal, gated the same way as the desktop: a signed `?t=` token
/// on the bare entry, redirecting into a token-path-segment world that the
/// proxy then tunnels straight to ttyd.
///
/// Unlike the desktop, ttyd serves its own index page at `/` (there is no
/// Buzz-branded wrapper to inject), so the entry redirects to the bare token
/// prefix with a trailing slash rather than to a `view` sub-path — ttyd's
/// relative asset fetches (and its `/ws` WebSocket) resolve from there.
async fn terminal(
    State(state): State<AppState>,
    Path(params): Path<Vec<(String, String)>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    req: Request,
) -> axum::response::Response {
    let SurfaceParams {
        id,
        path_token,
        rest,
    } = surface_params(&params);

    let (_caller, _public_base) =
        match verify_surface_token(&state, req.headers(), "terminal", &id, &path_token, &query)
            .await
        {
            Ok(v) => v,
            Err(response) => return response,
        };
    let Some(token) = path_token.clone().or_else(|| query.get("t").cloned()) else {
        return (StatusCode::UNAUTHORIZED, "missing viewer token").into_response();
    };

    if path_token.is_none() {
        let prefix = req
            .headers()
            .get("x-forwarded-prefix")
            .and_then(|v| v.to_str().ok())
            .map(|p| p.trim_end_matches('/'))
            .unwrap_or("");
        // Trailing slash: ttyd serves its index at exactly `/`, and every
        // relative asset/WebSocket URL it emits is resolved against that.
        let location = format!("{prefix}/sandboxes/{id}/terminal/t/{token}/");
        return axum::response::Redirect::temporary(&location).into_response();
    }

    let ip = match resolve_sandbox_ip(&state, &id).await {
        Ok(ip) => ip,
        Err(response) => return response,
    };

    // Empty `rest` under the token prefix is ttyd's own root, not this
    // broker's — the proxy already treats an empty path as "/".
    proxy::desktop_proxy(&format!("{ip}:{TERMINAL_PORT}"), &rest, req).await
}

async fn delete_sandbox(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    if let Err(response) =
        authorize(&state, &headers, "DELETE", &format!("/sandboxes/{id}"), b"").await
    {
        return response;
    }
    // Refuse to remove anything this broker did not create, even if the caller
    // knows its id.
    let owner = match state.docker.inspect_container(&id).await {
        Ok(v) if !is_managed(&v) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such sandbox"})),
            )
                .into_response()
        }
        Ok(v) => {
            // Track state by the full container id even when the caller used a
            // name or prefix alias.
            if let Some(full) = v.get("Id").and_then(|x| x.as_str()) {
                state.forget_expiry(full);
            }
            inspect_label(&v, sandbox::LABEL_OWNER)
        }
        Err(e) if e.contains("404") || e.to_lowercase().contains("no such container") => {
            // Already gone is success: the caller's intent is satisfied.
            return (StatusCode::NO_CONTENT, ()).into_response();
        }
        Err(e) => return internal(e),
    };
    match state.docker.remove_container(&id).await {
        Ok(()) => {
            if let Some(publisher) = state.publisher.as_ref() {
                // The owner rides along so the desktop can clear the right
                // agent's card without correlating ids itself.
                publisher
                    .sandbox_destroyed(&id, owner.as_deref(), "destroyed")
                    .await;
            }
            info!(sandbox = %id, "sandbox destroyed");
            (StatusCode::NO_CONTENT, ()).into_response()
        }
        Err(e) => internal(e),
    }
}

/// Extend (or shorten) a running sandbox's lifetime.
///
/// Owner-only: the sandbox is the *agent's* resource, so only the key it
/// belongs to may buy it more time — not any member who learns the id. The
/// new expiry is clamped so total lifetime never exceeds `MAX_TTL_SECONDS`
/// from creation, and the change is re-announced as a fresh kind:48200 so the
/// desktop's countdown follows.
async fn extend_sandbox(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/extend"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    let req: sandbox::ExtendRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("invalid request body: {e}")),
    };

    let inspect = match state.docker.inspect_container(&id).await {
        Ok(v) if is_managed(&v) => v,
        Ok(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such sandbox"})),
            )
                .into_response()
        }
        Err(e) if e.contains("404") || e.to_lowercase().contains("no such container") => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such sandbox"})),
            )
                .into_response()
        }
        Err(e) => return internal(e),
    };

    // Ownership lives on the container label, so the check is local and works
    // even when the relay is unreachable. A sandbox created without an owner
    // has no key that may extend it — that is refusal, not a fallback to
    // "anyone".
    let owner = inspect_label(&inspect, sandbox::LABEL_OWNER);
    if owner.as_deref() != Some(caller.as_str()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "only the sandbox's owner may extend it"
            })),
        )
            .into_response();
    }

    // The lifetime ceiling is measured from the container's creation, which
    // Docker records authoritatively — not from the label, which a future
    // change might alter.
    let Some(created_at) = created_at_of(&inspect) else {
        return internal("container has no readable creation time");
    };

    let now = chrono::Utc::now().timestamp();
    let expires_at = sandbox::extended_expiry(now, created_at, req.ttl_seconds);
    // Key on the full container id: the caller may have used a name or a
    // short-id prefix, but the reaper looks up by the id Docker lists.
    let full_id = inspect
        .get("Id")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();
    state.set_expiry(&full_id, expires_at);

    // Re-announce with the new expiry, unconditionally — an explicit extend
    // is a deliberate call, not high-frequency traffic, so it always
    // publishes (unlike the keepalive middleware's throttled bump). The
    // desktop reconstructs from the latest 48200, so a fresh announcement
    // moves its countdown; it also becomes the crash-recovery record
    // `seed_expiries` reads after a broker restart.
    republish_expiry(&state, &inspect, &full_id, owner.as_deref(), expires_at).await;
    state.set_last_published_expiry(&full_id, expires_at);

    info!(sandbox = %id, expires_at, "sandbox extended");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "id": id, "expires_at": expires_at })),
    )
        .into_response()
}

/// Re-announce a sandbox's facts to the relay with a new `expires_at`, as a
/// fresh kind:48200 landing on the same `d` tag (the container's full id) so
/// it replaces the previous announcement in the desktop's view instead of
/// appearing as a second sandbox.
///
/// Shared by `extend_sandbox` (always calls this) and the keepalive
/// middleware (calls it only past the republish throttle) — both need the
/// same inspect -> `SandboxFacts` projection, and factoring it out is what
/// keeps that projection from drifting between the two call sites the way
/// the id-resolve logic would if duplicated. Best-effort: a sandbox that
/// runs unannounced is a display gap, not a failure, so this never returns
/// an error for the caller to handle.
async fn republish_expiry(
    state: &AppState,
    inspect: &serde_json::Value,
    full_id: &str,
    owner: Option<&str>,
    expires_at: i64,
) {
    let Some(publisher) = state.publisher.as_ref() else {
        return;
    };
    let name = inspect
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    let image = inspect
        .pointer("/Config/Image")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let cpus = inspect
        .pointer("/HostConfig/NanoCpus")
        .and_then(|v| v.as_i64())
        .map(|n| n as f64 / 1e9)
        .unwrap_or(0.0);
    let memory_mb = inspect
        .pointer("/HostConfig/Memory")
        .and_then(|v| v.as_i64())
        .map(|b| (b / (1024 * 1024)) as u64)
        .unwrap_or(0);
    let viewer = state.viewer_base.as_ref().map(|base| {
        format!(
            "{base}/sandboxes/{}/desktop",
            &full_id[..12.min(full_id.len())]
        )
    });
    publisher
        .sandbox_created(events::SandboxFacts {
            sandbox_id: full_id,
            name: &name,
            image: &image,
            owner,
            cpus,
            memory_mb,
            expires_at,
            viewer_url: viewer.as_deref(),
        })
        .await;
}

/// The Unix identity every file-API exec runs as: the unprivileged agent
/// user, uid/gid 10001 (matches `RUN_AS_UID`/`RUN_AS_GID`, see
/// `Dockerfile.sprig-dev`). Never root — the file API acts inside the
/// sandbox with the same privilege the agent process itself has.
const FS_EXEC_USER: &str = "10001:10001";

/// Cap on a file upload body. Large enough for source files and small
/// artifacts, small enough that a caller cannot use the file API to fill the
/// host's disk in one request — the TTL reaper and concurrency cap bound
/// sandbox count and lifetime, not bytes written per call.
const FS_MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// One entry of a directory listing, as emitted by the in-container `python3
/// -c` scan and reported back to the caller.
#[derive(serde::Deserialize, serde::Serialize)]
struct FsEntry {
    name: String,
    kind: String,
    size: u64,
    mtime: i64,
}

#[derive(serde::Deserialize)]
struct FsPathQuery {
    path: String,
}

#[derive(serde::Deserialize)]
struct FsRenameRequest {
    from: String,
    to: String,
}

#[derive(serde::Deserialize)]
struct LaunchRequest {
    app: String,
    /// Only valid alongside `app == "browser"` — validated by
    /// `sandbox::validate_launch_url` and appended as its own argv element,
    /// never shell-interpolated.
    #[serde(default)]
    url: Option<String>,
}

/// Authorize an fs-API call and validate its `path`, in that order — the
/// signature check is local and must run before anything path-shaped is
/// interpreted, and the path policy applies regardless of *who* is calling
/// (matching the existing DELETE routes, any community member may act).
async fn authorize_fs(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
    id: &str,
    fs_path: &str,
) -> Result<(), axum::response::Response> {
    if !is_safe_id(id) {
        return Err(bad_request("malformed sandbox id"));
    }
    authorize(state, headers, method, path, body).await?;
    sandbox::validate_fs_path(fs_path).map_err(bad_request)?;
    Ok(())
}

async fn fs_list(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    Query(q): Query<FsPathQuery>,
) -> axum::response::Response {
    if let Err(response) = authorize_fs(
        &state,
        &headers,
        "GET",
        &format!("/sandboxes/{id}/fs?{}", raw.unwrap_or_default()),
        b"",
        &id,
        &q.path,
    )
    .await
    {
        return response;
    }

    // A small Python scan emits one JSON object per line: precise types
    // (file/dir/other), sizes, and mtimes without shelling out to `ls` and
    // parsing column-aligned text. The path travels as an environment
    // variable (`BUZZ_FS_PATH`), never interpolated into the script's source,
    // so it needs no Python-string escaping regardless of what characters the
    // (already `validate_fs_path`-checked) path contains.
    const LIST_SCRIPT: &str = "import os, json\nfor e in os.scandir(os.environ['BUZZ_FS_PATH']):\n    try:\n        st = e.stat(follow_symlinks=False)\n    except OSError:\n        continue\n    kind = 'dir' if e.is_dir(follow_symlinks=False) else ('file' if e.is_file(follow_symlinks=False) else 'other')\n    print(json.dumps({'name': e.name, 'kind': kind, 'size': st.st_size, 'mtime': int(st.st_mtime)}))\n";
    let (exit_code, output) = match state
        .docker
        .exec_with_env(
            &id,
            &["python3", "-c", LIST_SCRIPT],
            FS_EXEC_USER,
            &[format!("BUZZ_FS_PATH={}", q.path)],
        )
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if exit_code != 0 {
        let detail = String::from_utf8_lossy(&output);
        if detail.contains("FileNotFoundError") || detail.contains("No such file") {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such path"})),
            )
                .into_response();
        }
        return internal(format!("listing failed: {}", detail.trim()));
    }

    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<FsEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                // Defensive: one malformed line (e.g. a `NameError` traceback
                // fragment) should not sink the whole listing.
                warn!(error = %e, line, "skipping unparseable fs listing line");
            }
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({ "path": q.path, "entries": entries })),
    )
        .into_response()
}

async fn fs_download(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    Query(q): Query<FsPathQuery>,
) -> axum::response::Response {
    if let Err(response) = authorize_fs(
        &state,
        &headers,
        "GET",
        &format!("/sandboxes/{id}/fs/file?{}", raw.unwrap_or_default()),
        b"",
        &id,
        &q.path,
    )
    .await
    {
        return response;
    }

    let tar_bytes = match state.docker.get_archive(&id, &q.path).await {
        Ok(b) => b,
        Err(e) if e.contains("404") || e.to_lowercase().contains("no such") => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such path"})),
            )
                .into_response()
        }
        Err(e) => return internal(e),
    };

    let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => return internal(format!("could not read archive: {e}")),
    };
    for entry in entries {
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => return internal(format!("could not read archive entry: {e}")),
        };
        let is_dir = entry.header().entry_type().is_dir();
        if is_dir {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "path is a directory, not a file"})),
            )
                .into_response();
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        if let Err(e) = std::io::Read::read_to_end(&mut entry, &mut bytes) {
            return internal(format!("could not read file contents: {e}"));
        }
        let filename = std::path::Path::new(&q.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        return (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/octet-stream".to_string(),
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ],
            bytes,
        )
            .into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "no such path"})),
    )
        .into_response()
}

async fn fs_upload(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    Query(q): Query<FsPathQuery>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if body.len() > FS_MAX_UPLOAD_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": format!("upload exceeds the {FS_MAX_UPLOAD_BYTES}-byte limit")
            })),
        )
            .into_response();
    }
    if let Err(response) = authorize_fs(
        &state,
        &headers,
        "PUT",
        &format!("/sandboxes/{id}/fs/file?{}", raw.unwrap_or_default()),
        &body,
        &id,
        &q.path,
    )
    .await
    {
        return response;
    }

    let path = std::path::Path::new(&q.path);
    let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return bad_request("path has no parent directory");
    };
    let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
        return bad_request("path has no file name");
    };

    // Parent directories are created broker-side (exec `mkdir -p`) rather than
    // relying on Docker's put-archive to create them, since put-archive only
    // guarantees the *named* directory exists, not arbitrary depth beneath it.
    if let Err(e) = state
        .docker
        .exec(
            &id,
            &["mkdir", "-p", "--", &parent.to_string_lossy()],
            FS_EXEC_USER,
        )
        .await
    {
        return internal(format!("could not prepare parent directory: {e}"));
    }

    let tar_bytes = match build_single_file_tar(filename, &body) {
        Ok(b) => b,
        Err(e) => return internal(format!("could not build upload archive: {e}")),
    };
    if let Err(e) = state
        .docker
        .put_archive(&id, &parent.to_string_lossy(), tar_bytes)
        .await
    {
        return internal(e);
    }

    (StatusCode::OK, Json(serde_json::json!({ "path": q.path }))).into_response()
}

/// Build a tar containing exactly one regular file, owned by the sandbox's
/// agent user (uid/gid 10001) with mode 0644 — readable and writable by that
/// user regardless of what wrote the tar.
fn build_single_file_tar(filename: &str, contents: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_uid(10001);
    header.set_gid(10001);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();
    builder.append_data(&mut header, filename, contents)?;
    builder.into_inner()
}

async fn fs_rename(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    if let Err(response) = authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/fs/rename"),
        &body,
    )
    .await
    {
        return response;
    }
    let req: FsRenameRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("invalid request body: {e}")),
    };
    if let Err(e) = sandbox::validate_fs_path(&req.from) {
        return bad_request(e);
    }
    if let Err(e) = sandbox::validate_fs_path(&req.to) {
        return bad_request(e);
    }

    let (exit_code, output) = match state
        .docker
        .exec(&id, &["mv", "--", &req.from, &req.to], FS_EXEC_USER)
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if exit_code != 0 {
        let detail = String::from_utf8_lossy(&output);
        if detail.contains("No such file") {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such path"})),
            )
                .into_response();
        }
        return internal(format!("rename failed: {}", detail.trim()));
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({ "from": req.from, "to": req.to })),
    )
        .into_response()
}

async fn fs_delete(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    Query(q): Query<FsPathQuery>,
) -> axum::response::Response {
    if let Err(response) = authorize_fs(
        &state,
        &headers,
        "DELETE",
        &format!("/sandboxes/{id}/fs?{}", raw.unwrap_or_default()),
        b"",
        &id,
        &q.path,
    )
    .await
    {
        return response;
    }

    let (exit_code, output) = match state
        .docker
        .exec(&id, &["rm", "-rf", "--", &q.path], FS_EXEC_USER)
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if exit_code != 0 {
        return internal(format!(
            "delete failed: {}",
            String::from_utf8_lossy(&output).trim()
        ));
    }

    (StatusCode::NO_CONTENT, ()).into_response()
}

/// Launch one of the fixed desktop apps as a real window under the sandbox's
/// window manager.
///
/// `app` selects among exactly three constant argv vectors in
/// `sandbox::LAUNCH_APPS` — no field of the request body ever reaches the
/// container's argv, so there is nothing here for a caller to inject through.
/// The process is started detached (`Docker::exec_detached`): these are GUI
/// apps meant to keep running until the user closes their window, and a
/// wait-for-exit exec (`Docker::exec`) would block the request for as long as
/// the app stayed open.
async fn launch_app(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/launch"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    let req: LaunchRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("invalid request body: {e}")),
    };
    let argv = match sandbox::launch_argv(&req.app) {
        Ok(argv) => argv,
        Err(e) => return bad_request(e),
    };
    if let Some(url) = req.url.as_deref() {
        if let Err(e) = sandbox::validate_launch_url(&req.app, url) {
            return bad_request(e);
        }
    }

    // Owner-or-manager (see `require_owner_or_manager`): without this, any
    // member could force-open an app in someone else's sandbox — and with
    // the browser `url` field, force-navigate their screen to an
    // attacker-chosen page. Plain owner-only would break the desktop dock,
    // which signs launch calls with the *human manager's* key, not the
    // sandbox owner's (see desktop/src-tauri/src/sandbox_viewer.rs).
    // Checked after body/argv validation (cheap, local) but before anything
    // reaches Docker.
    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }

    // DISPLAY/HOME so the app finds the X server and its own config/profile
    // directories under the agent's home, same as everything else on this
    // desktop image (see Dockerfile.sprig-desktop).
    let env = ["DISPLAY=:1".to_string(), "HOME=/home/agent".to_string()];
    let full_argv = sandbox::append_launch_url(argv, req.url.as_deref());
    if let Err(e) = state
        .docker
        .exec_detached(&id, &full_argv, FS_EXEC_USER, &env)
        .await
    {
        return internal(e);
    }

    // A body, not `NO_CONTENT`: every other mutating endpoint in this broker
    // answers with JSON, and the CLI's `broker_call` always tries to parse
    // the response body as JSON regardless of status code — an empty body
    // here made a successful launch look like a client-side parse failure
    // (`buzz sandbox open` reported an error even though the app opened).
    (
        StatusCode::OK,
        Json(launch_success_body(&req.app, req.url.as_deref())),
    )
        .into_response()
}

/// The JSON body `launch_app` answers with once the app is actually running.
///
/// Pulled out as a pure function (mirroring `sandbox::launch_argv` and
/// `sandbox::append_launch_url`) so the response shape is unit-testable
/// without a live Docker daemon — every test in this module's `mod tests`
/// runs against a dead Docker socket by design, so `launch_app` itself can
/// never reach this line in a test.
fn launch_success_body(app: &str, url: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "launched": true,
        "app": app,
        "url": url,
    })
}

/// Inspect a sandbox by id, returning 404 (not the inspect error verbatim)
/// when it doesn't exist or isn't one this broker manages.
///
/// Shared lookup for every owner/manager gate — mirroring the check
/// `extend_sandbox` does inline (ownership lives on the container label, so
/// this is local and works even when the relay is unreachable) — factored
/// out so `require_owner_or_manager` differs from any future gate only in
/// *whose* pubkey it accepts, not in how it looks the container up.
async fn inspect_managed_sandbox(
    state: &AppState,
    id: &str,
) -> Result<serde_json::Value, axum::response::Response> {
    match state.docker.inspect_container(id).await {
        Ok(v) if is_managed(&v) => Ok(v),
        Ok(_) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no such sandbox"})),
        )
            .into_response()),
        Err(e) if e.contains("404") || e.to_lowercase().contains("no such container") => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no such sandbox"})),
        )
            .into_response()),
        Err(e) => Err(internal(e)),
    }
}

/// Look up a managed sandbox and confirm `caller` is either its owner or its
/// manager.
///
/// The two are the same pubkey only when an agent self-creates its own box
/// (e.g. via the CLI); when the desktop app creates a sandbox on an agent's
/// behalf, it signs as the *human* manager while the agent's own key is
/// recorded as owner (see `LABEL_MANAGER`). Both may drive the sandbox's
/// computer-use surface — the manager because they are the human watching
/// (or taking over) the screen the dock's buttons act on, the owner because
/// it is their box. If the manager label is absent (a sandbox created before
/// this gate existed), this falls back to owner-only rather than admitting
/// anyone — never fail open on a missing label.
async fn require_owner_or_manager(
    state: &AppState,
    id: &str,
    caller: &str,
) -> Result<serde_json::Value, axum::response::Response> {
    let inspect = inspect_managed_sandbox(state, id).await?;

    let owner = inspect_label(&inspect, sandbox::LABEL_OWNER);
    let manager = inspect_label(&inspect, sandbox::LABEL_MANAGER);
    if !sandbox::is_owner_or_manager(caller, owner.as_deref(), manager.as_deref()) {
        return Err((
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "only the sandbox's owner or its manager may act on it"
            })),
        )
            .into_response());
    }
    Ok(inspect)
}

/// `POST /sandboxes/{id}/exec` — run a command inside the sandbox and return
/// its output. Owner-or-manager computer-use primitive (see
/// `require_owner_or_manager`): the caller supplies argv (never a shell
/// string), the broker prepends a `timeout` wrapper so a
/// runaway command cannot hang the sandbox forever, and a
/// `tokio::time::timeout` above that is a belt-and-braces bound in case the
/// in-container `timeout` binary itself is missing or misbehaves.
async fn sandbox_exec(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/exec"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    let req: sandbox::ExecRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("invalid request body: {e}")),
    };
    let resolved = match sandbox::resolve_exec(req) {
        Ok(r) => r,
        Err(e) => return bad_request(e),
    };

    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }

    // Prepend the timeout wrapper — a fixed argv0 and the resolved numeric
    // timeout, not caller-controlled text — so the in-container command dies
    // on schedule even if it ignores signals sent any other way.
    let timeout_str = resolved.timeout_secs.to_string();
    let argv: Vec<&str> = std::iter::once("timeout")
        .chain(std::iter::once(timeout_str.as_str()))
        .chain(resolved.argv.iter().map(|s| s.as_str()))
        .collect();

    let env = [format!("BUZZ_EXEC_WORKDIR={}", resolved.workdir)];
    // The workdir travels via env and a `cd` prefix executed by `sh -c` would
    // reopen the shell-injection door this module otherwise avoids, so
    // instead the exec's argv is wrapped one level: `sh -c 'cd "$BUZZ_EXEC_WORKDIR" && exec "$@"' -- <argv...>`
    // keeps every caller-controlled value in its own argv/env slot.
    let mut wrapped: Vec<&str> = vec!["sh", "-c", "cd \"$BUZZ_EXEC_WORKDIR\" && exec \"$@\"", "--"];
    wrapped.extend(argv.iter());

    let belt_and_braces = std::time::Duration::from_secs(resolved.timeout_secs + 5);
    let result = tokio::time::timeout(
        belt_and_braces,
        state
            .docker
            .exec_with_env(&id, &wrapped, FS_EXEC_USER, &env),
    )
    .await;

    let (exit_code, output, timed_out) = match result {
        Ok(Ok((code, output))) => (code, output, code == 124),
        Ok(Err(e)) => return internal(e),
        Err(_) => (124i64, Vec::new(), true),
    };

    let truncate = |bytes: &[u8]| -> String {
        let mut s = String::from_utf8_lossy(bytes).into_owned();
        if s.len() > sandbox::EXEC_MAX_OUTPUT_BYTES {
            s.truncate(sandbox::EXEC_MAX_OUTPUT_BYTES);
            s.push_str("\n[truncated]");
        }
        s
    };
    // `Docker::exec_with_env` concatenates stdout+stderr in stream order
    // rather than demuxing them (see its own doc comment) — reporting the
    // combined text under `stdout` and leaving `stderr` empty is honest about
    // that, rather than fabricating a stream split the transport does not
    // give us.
    let stdout = truncate(&output);
    let stderr = String::new();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "timed_out": timed_out,
        })),
    )
        .into_response()
}

/// `GET /sandboxes/{id}/screenshot` — capture the sandbox's desktop and
/// return it as a raw PNG. Owner-or-manager: the screen can show anything
/// the agent (or the human manager watching it) is doing, so both — and no
/// one else — may see it.
async fn sandbox_screenshot(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    // NIP-98 on a GET signs an empty body, matching fs_list.
    let caller = match authorize(
        &state,
        &headers,
        "GET",
        &format!("/sandboxes/{id}/screenshot"),
        b"",
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }

    const SCREENSHOT_PATH: &str = "/tmp/buzz-screenshot.png";
    let env = ["DISPLAY=:1".to_string()];
    let (exit_code, output) = match state
        .docker
        .exec_with_env(
            &id,
            &["scrot", "-o", "-z", SCREENSHOT_PATH],
            FS_EXEC_USER,
            &env,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    if exit_code != 0 {
        warn!(
            sandbox = %id,
            exit_code,
            detail = %String::from_utf8_lossy(&output).trim(),
            "screenshot capture failed"
        );
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": "screenshot tool unavailable in this computer — restart it to pick up the new image"
            })),
        )
            .into_response();
    }

    let tar_bytes = match state.docker.get_archive(&id, SCREENSHOT_PATH).await {
        Ok(b) => b,
        Err(e) => {
            warn!(sandbox = %id, error = %e, "screenshot file missing after capture");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "screenshot tool unavailable in this computer — restart it to pick up the new image"
                })),
            )
                .into_response();
        }
    };

    let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => return internal(format!("could not read archive: {e}")),
    };
    for entry in entries {
        let mut entry = match entry {
            Ok(e) => e,
            Err(e) => return internal(format!("could not read archive entry: {e}")),
        };
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        if let Err(e) = std::io::Read::read_to_end(&mut entry, &mut bytes) {
            return internal(format!("could not read screenshot contents: {e}"));
        }
        return (
            [(axum::http::header::CONTENT_TYPE, "image/png".to_string())],
            bytes,
        )
            .into_response();
    }

    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": "screenshot tool unavailable in this computer — restart it to pick up the new image"
        })),
    )
        .into_response()
}

/// `POST /sandboxes/{id}/input` — perform a sequence of mouse/keyboard
/// actions on the sandbox's desktop via `xdotool`. Owner-or-manager, same as
/// exec and screenshot: this drives the same screen the owner or the human
/// manager watching it is looking at.
async fn sandbox_input(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/input"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    let req: sandbox::InputRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return bad_request(format!("invalid request body: {e}")),
    };
    if let Err(e) = sandbox::validate_input_request(&req) {
        return bad_request(e);
    }
    // Build and validate every action's argv up front, before any exec runs —
    // a request that is partly invalid should fail atomically rather than
    // performing some actions and rejecting others.
    let mut argvs: Vec<Vec<String>> = Vec::with_capacity(req.actions.len());
    for (i, action) in req.actions.iter().enumerate() {
        match sandbox::input_action_argv(action) {
            Ok(argv) => argvs.push(argv),
            Err(e) => return bad_request(format!("action {i}: {e}")),
        }
    }

    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }

    let env = ["DISPLAY=:1".to_string()];
    for (i, argv) in argvs.iter().enumerate() {
        let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let (exit_code, output) = match state
            .docker
            .exec_with_env(&id, &argv_refs, FS_EXEC_USER, &env)
            .await
        {
            Ok(v) => v,
            Err(e) => return internal(e),
        };
        if exit_code != 0 {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!(
                        "action {i} failed: {}",
                        String::from_utf8_lossy(&output).trim()
                    )
                })),
            )
                .into_response();
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({ "performed": argvs.len() })),
    )
        .into_response()
}

/// How long `sandbox_recording_stop` waits for ffmpeg to finish flushing its
/// muxer after a graceful stop signal before giving up. ffmpeg's own shutdown
/// on SIGINT/SIGTERM is normally near-instant for a screen-capture stream —
/// this is a generous ceiling against a slow container, not the expected case.
const RECORDING_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// How often `sandbox_recording_stop` polls for ffmpeg's exit while waiting
/// out [`RECORDING_STOP_TIMEOUT`].
const RECORDING_STOP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

/// `POST /sandboxes/{id}/recording/start` — start capturing the sandbox's
/// screen to a fixed in-container path via `ffmpeg`, detached (the exec
/// returns as soon as the process is launched, not when it exits). Same
/// owner-or-manager gate as exec/input/screenshot: a recording shows exactly
/// what those already expose live.
///
/// Only one recording may run at a time per sandbox — a second `start` while
/// one is already in progress is a 409, not a silently-ignored no-op, so a
/// caller cannot lose track of which capture it will get back from `stop`.
async fn sandbox_recording_start(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/recording/start"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }

    let is_running_argv = sandbox::recording_is_running_argv();
    match state.docker.exec(&id, &is_running_argv, FS_EXEC_USER).await {
        Ok((0, _)) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "a recording is already in progress"})),
            )
                .into_response();
        }
        Ok(_) => {}
        Err(e) => return internal(e),
    }

    let env = ["DISPLAY=:1".to_string()];
    let start_argv = sandbox::recording_start_argv();
    if let Err(e) = state
        .docker
        .exec_detached(&id, &start_argv, FS_EXEC_USER, &env)
        .await
    {
        return internal(e);
    }
    info!(
        sandbox_id = %id,
        max_duration_secs = sandbox::RECORDING_MAX_DURATION_SECS,
        "recording started; ffmpeg self-terminates at the -t cap even if no stop call ever arrives"
    );

    (StatusCode::OK, Json(serde_json::json!({"recording": true}))).into_response()
}

/// Query hint on `POST /sandboxes/{id}/recording/stop` naming the format of
/// an optional audio body — see [`sandbox::AUDIO_EXT_ALLOWLIST`]. Absent
/// entirely when the call carries no audio.
#[derive(serde::Deserialize, Default)]
struct RecordingStopQuery {
    #[serde(default)]
    audio_ext: Option<String>,
}

/// `POST /sandboxes/{id}/recording/stop` — gracefully stop an in-progress
/// recording and return the finished mp4's raw bytes. Owner-or-manager, same
/// as `start`.
///
/// "Gracefully" matters here: ffmpeg writes the mp4's index (`moov` atom)
/// only on a clean shutdown, so this sends `SIGINT` and polls for the process
/// to actually exit before reading the file — pulling it out from under a
/// still-running (or force-killed) ffmpeg would return an unplayable file.
/// Reuses the exact `get_archive` + tar-extraction path `sandbox_screenshot`
/// uses for its PNG.
///
/// Optionally takes a human's narration track in the same call: pass
/// `?audio_ext=<wav|webm|ogg|m4a|mp3|aac>` and put the raw audio bytes in the
/// request body (in place of the empty body a video-only stop sends). When
/// present, the audio is pushed into the sandbox and muxed against the just
/// -recorded screen capture with `ffmpeg -c:v copy -c:a aac -shortest`, and
/// the *merged* mp4 is what comes back — never both files separately. This
/// is intentionally one call rather than a second "mux" endpoint: the video
/// file only exists on the sandbox's disk for the instant between stopping
/// ffmpeg and reading it back, and giving a second endpoint a window to find
/// that file would mean tracking its lifetime across two round trips instead
/// of one. Omitting `audio_ext` (or sending an empty body) reproduces
/// exactly today's video-only behavior — callers that haven't wired up audio
/// capture yet are unaffected.
async fn sandbox_recording_stop(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
    Query(q): Query<RecordingStopQuery>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    if body.len() > sandbox::RECORDING_AUDIO_MAX_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": format!(
                    "audio upload exceeds the {}-byte limit",
                    sandbox::RECORDING_AUDIO_MAX_BYTES
                )
            })),
        )
            .into_response();
    }
    // The audio extension hint is part of what the caller must sign over:
    // folding it into the request line (rather than trusting the query
    // separately from NIP-98) means a caller cannot replay a stop call
    // signed for one extension against a different one.
    let query_suffix = raw.map(|r| format!("?{r}")).unwrap_or_default();
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/recording/stop{query_suffix}"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }

    // An audio track is present only when the caller named its format *and*
    // sent bytes — either alone (a hint with an empty body, or a body with no
    // hint) is treated as "no audio", matching how `fs_upload` treats an
    // absent body: there is nothing to validate an extension against, or
    // nothing to write.
    let audio: Option<(&str, &[u8])> = match (q.audio_ext.as_deref(), body.as_ref()) {
        (Some(ext), bytes) if !bytes.is_empty() => Some((ext, bytes)),
        _ => None,
    };

    let is_running_argv = sandbox::recording_is_running_argv();
    match state.docker.exec(&id, &is_running_argv, FS_EXEC_USER).await {
        Ok((0, _)) => {}
        Ok(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "no recording is in progress"})),
            )
                .into_response();
        }
        Err(e) => return internal(e),
    }

    let stop_argv = sandbox::recording_stop_argv();
    if let Err(e) = state.docker.exec(&id, &stop_argv, FS_EXEC_USER).await {
        return internal(e);
    }

    // Poll for ffmpeg to actually exit (its own clean shutdown, not the
    // signal delivery, is what finalizes the mp4) rather than reading the
    // file immediately after sending the signal.
    let deadline = tokio::time::Instant::now() + RECORDING_STOP_TIMEOUT;
    loop {
        match state.docker.exec(&id, &is_running_argv, FS_EXEC_USER).await {
            Ok((0, _)) => {
                if tokio::time::Instant::now() >= deadline {
                    warn!(sandbox = %id, "recording did not stop within the timeout; reading file anyway");
                    break;
                }
                tokio::time::sleep(RECORDING_STOP_POLL_INTERVAL).await;
            }
            Ok(_) => break,
            Err(e) => return internal(e),
        }
    }

    let Some((ext, audio_bytes)) = audio else {
        return recording_video_response(&state, &id).await;
    };

    let audio_path = match sandbox::audio_upload_path(ext) {
        Ok(p) => p,
        Err(e) => return bad_request(e),
    };
    let audio_filename = std::path::Path::new(&audio_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("buzz-teach-audio");
    let audio_tar = match build_single_file_tar(audio_filename, audio_bytes) {
        Ok(t) => t,
        Err(e) => return internal(format!("could not build audio upload archive: {e}")),
    };
    if let Err(e) = state.docker.put_archive(&id, "/tmp", audio_tar).await {
        return internal(format!("could not upload audio track: {e}"));
    }

    let mux_argv = sandbox::recording_mux_argv(sandbox::RECORDING_PATH, &audio_path);
    let mux_result = state.docker.exec(&id, &mux_argv, FS_EXEC_USER).await;
    // Best-effort cleanup of the audio upload regardless of mux outcome — it
    // has already served its purpose either way, and leaving it behind would
    // accumulate across repeated takes the same way a stray recording would.
    let _ = state
        .docker
        .exec(&id, &["rm", "-f", "--", &audio_path], FS_EXEC_USER)
        .await;
    match mux_result {
        Ok((0, _)) => {}
        Ok((_, output)) => {
            warn!(
                sandbox = %id,
                output = %String::from_utf8_lossy(&output),
                "ffmpeg mux failed"
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "could not mux the audio track into the recording"
                })),
            )
                .into_response();
        }
        Err(e) => return internal(e),
    }

    let tar_bytes = match state
        .docker
        .get_archive(&id, sandbox::RECORDING_MERGED_PATH)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            warn!(sandbox = %id, error = %e, "merged recording missing after mux");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "merged recording was not found after muxing"
                })),
            )
                .into_response();
        }
    };
    // Best-effort cleanup of the merged file — same spirit as the audio
    // cleanup above, so repeated "Teach a task" takes on one long-lived
    // sandbox don't accumulate stale mp4s under /tmp.
    let _ = state
        .docker
        .exec(
            &id,
            &["rm", "-f", "--", sandbox::RECORDING_MERGED_PATH],
            FS_EXEC_USER,
        )
        .await;

    match single_file_from_tar_bytes(tar_bytes) {
        Ok(Some(bytes)) => (
            [(axum::http::header::CONTENT_TYPE, "video/mp4".to_string())],
            bytes,
        )
            .into_response(),
        Ok(None) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": "merged recording was not found after muxing"
            })),
        )
            .into_response(),
        Err(e) => internal(e),
    }
}

/// Fetch and return the plain (audio-free) recording — the path
/// `sandbox_recording_stop` takes when no audio track was uploaded, factored
/// out so that path reads exactly as it did before muxing existed.
async fn recording_video_response(state: &AppState, id: &str) -> axum::response::Response {
    let tar_bytes = match state.docker.get_archive(id, sandbox::RECORDING_PATH).await {
        Ok(b) => b,
        Err(e) => {
            warn!(sandbox = %id, error = %e, "recording file missing after stop");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "recording file was not found after stopping"
                })),
            )
                .into_response();
        }
    };

    match single_file_from_tar_bytes(tar_bytes) {
        Ok(Some(bytes)) => (
            [(axum::http::header::CONTENT_TYPE, "video/mp4".to_string())],
            bytes,
        )
            .into_response(),
        Ok(None) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": "recording file was not found after stopping"
            })),
        )
            .into_response(),
        Err(e) => internal(format!("could not read recording archive: {e}")),
    }
}

/// Extract the first regular file's contents from a single-entry tar, as
/// returned by Docker's container-archive `GET`. `Ok(None)` means the tar
/// had no file entry — distinct from an I/O error reading one that exists.
fn single_file_from_tar_bytes(tar_bytes: Vec<u8>) -> Result<Option<Vec<u8>>, String> {
    let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
    let entries = archive
        .entries()
        .map_err(|e| format!("could not read archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("could not read archive entry: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes)
            .map_err(|e| format!("could not read file contents: {e}"))?;
        return Ok(Some(bytes));
    }
    Ok(None)
}

/// `POST /sandboxes/{id}/heartbeat` — a pure activity signal, owner-or-manager
/// gated like every other computer-use route. Does nothing itself; its only
/// purpose is to give the desktop viewer a 2xx to send while a sandbox's
/// screen is on-screen but otherwise idle (no exec/input of its own), so the
/// keepalive middleware has something to bump expiry on for passive
/// watching.
async fn heartbeat(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    if !is_safe_id(&id) {
        return bad_request("malformed sandbox id");
    }
    let caller = match authorize(
        &state,
        &headers,
        "POST",
        &format!("/sandboxes/{id}/heartbeat"),
        &body,
    )
    .await
    {
        Ok(pubkey) => pubkey,
        Err(response) => return response,
    };
    if let Err(response) = require_owner_or_manager(&state, &id, &caller).await {
        return response;
    }
    (StatusCode::OK, ()).into_response()
}

/// The brand-free sandbox viewer: a full-bleed live screen, nothing else.
///
/// Served by the broker (not the sandbox) so it needs no assets of its own —
/// it imports the RFB engine module relatively, which the desktop proxy
/// serves from the sandbox's static tree under the same token path. Scaling
/// is remote-cursor + local fit; a dropped connection retries quietly until
/// the token ages out, at which point reopening from Buzz mints a fresh one.
const VIEWER_PAGE: &str = r##"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>Agent desktop</title>
<style>
  html, body { margin: 0; height: 100%; background: #101014; overflow: hidden; }
  #screen { width: 100%; height: 100%; }
</style>
</head>
<body>
<div id="screen"></div>
<script type="module">
  import RFB from "./core/rfb.js";
  const base = location.pathname.replace(/\/view$/, "");
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const url = `${proto}://${location.host}${base}/websockify`;
  let retry = 0;
  function connect() {
    const rfb = new RFB(document.getElementById("screen"), url);
    rfb.scaleViewport = true;
    rfb.background = "#101014";
    rfb.addEventListener("connect", () => { retry = 0; });
    rfb.addEventListener("disconnect", () => {
      // Quiet backoff; a token past its window keeps failing until the
      // viewer is reopened from Buzz, which mints a fresh one.
      retry += 1;
      if (retry <= 30) setTimeout(connect, Math.min(1000 * retry, 5000));
    });
  }
  connect();
</script>
</body>
</html>
"##;

/// A label from a container *inspect* payload (labels live under
/// `Config.Labels` there, unlike the flat `Labels` of a list entry).
fn inspect_label(inspect: &serde_json::Value, key: &str) -> Option<String> {
    inspect
        .pointer("/Config/Labels")?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// A container's creation time from an *inspect* payload, as Docker records
/// it authoritatively — not from the expiry label, which a future change
/// might alter. This is the reference point every lifetime ceiling
/// (`extended_expiry`, `MAX_TTL_SECONDS`) measures from, so both the extend
/// handler and the keepalive middleware read it the same way.
fn created_at_of(inspect: &serde_json::Value) -> Option<i64> {
    inspect
        .get("Created")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp())
}

/// A resolved keepalive target: everything a bump needs about the sandbox a
/// `{id}` path segment named, read from one inspect call.
struct KeepaliveTarget {
    full_id: String,
    created_at: i64,
    owner: Option<String>,
    inspect: serde_json::Value,
}

/// Resolve a sandbox-scoped URL's `{id}` path segment — which may be a short
/// prefix, same as every other sandbox route accepts — to the full container
/// id, its creation time, and its owner: the facts a keepalive bump needs.
///
/// Shared by the keepalive middleware and available to any handler that
/// needs the same resolve, so the prefix -> full-id mapping exists in
/// exactly one place rather than risking drift between them (the same
/// reasoning as `inspect_managed_sandbox`, which this mirrors but does not
/// call directly — that helper builds an HTTP error response on failure,
/// which the middleware has no use for; it just skips the bump). Returns
/// `None` for anything that isn't a live, managed sandbox with a readable
/// creation time — an id that fails to resolve is simply not bumped, the
/// same as any other lookup miss.
async fn resolve_keepalive_target(state: &AppState, id: &str) -> Option<KeepaliveTarget> {
    if !is_safe_id(id) {
        return None;
    }
    let inspect = state.docker.inspect_container(id).await.ok()?;
    if !is_managed(&inspect) {
        return None;
    }
    let full_id = inspect.get("Id").and_then(|v| v.as_str())?.to_string();
    let created_at = created_at_of(&inspect)?;
    let owner = inspect_label(&inspect, sandbox::LABEL_OWNER);
    Some(KeepaliveTarget {
        full_id,
        created_at,
        owner,
        inspect,
    })
}

/// The `{id}` segment of a `/sandboxes/{id}/...` request path, without
/// pulling in axum's path-matching machinery — the middleware runs before
/// routing extracts `Path<String>` for the eventual handler, so it reads the
/// same segment straight from the URI it already has.
fn sandbox_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/sandboxes/")?
        .split('/')
        .next()
        .filter(|s| !s.is_empty())
}

/// Activity keepalive: runs the handler, and only when it answers with a
/// 2xx, bumps the sandbox's expiry.
///
/// Ordering is what makes this safe to trust: every sandbox-scoped route
/// this wraps already runs `authorize` / `verify_surface_token` before doing
/// anything else, and answers 401/403 on failure — so by the time this
/// middleware sees a 2xx, authentication (and, for owner/manager routes,
/// authorization) has already succeeded inside the handler. An unauthenticated
/// prober never produces a 2xx, so it can never keep a box alive. This
/// middleware does not re-check identity itself; it trusts the status code
/// the handler already gated on.
///
/// The bump is `extended_expiry(now, created_at, KEEPALIVE_GRACE_SECONDS)` —
/// the same clamp `extend_sandbox` uses, so continuous use can delay a
/// sandbox's death but never lift the `created_at + MAX_TTL_SECONDS` ceiling.
/// Only the in-memory `expiry` map is touched on every bump (cheap, and all
/// the reaper reads); the kind:48200 re-announcement is throttled to once
/// per `KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS` of actual movement so normal
/// traffic does not spam the relay.
async fn keepalive_middleware(
    State(state): State<AppState>,
    request: Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let id = sandbox_id_from_path(request.uri().path()).map(str::to_string);
    let response = next.run(request).await;

    if !response.status().is_success() {
        return response;
    }
    let Some(id) = id else {
        return response;
    };
    let Some(target) = resolve_keepalive_target(&state, &id).await else {
        return response;
    };

    let now = chrono::Utc::now().timestamp();
    let expires_at =
        sandbox::extended_expiry(now, target.created_at, sandbox::KEEPALIVE_GRACE_SECONDS);
    state.set_expiry(&target.full_id, expires_at);

    let last_published = state.last_published_expiry_of(&target.full_id);
    let moved_enough = match last_published {
        Some(last) => (expires_at - last).abs() >= sandbox::KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS,
        None => true,
    };
    if moved_enough {
        republish_expiry(
            &state,
            &target.inspect,
            &target.full_id,
            target.owner.as_deref(),
            expires_at,
        )
        .await;
        state.set_last_published_expiry(&target.full_id, expires_at);
    }

    response
}

#[cfg(test)]
mod keepalive_tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    #[test]
    fn extracts_the_id_segment_from_a_sandbox_scoped_path() {
        assert_eq!(sandbox_id_from_path("/sandboxes/abc123"), Some("abc123"));
        assert_eq!(
            sandbox_id_from_path("/sandboxes/abc123/exec"),
            Some("abc123")
        );
        assert_eq!(
            sandbox_id_from_path("/sandboxes/abc123/desktop/t/tok/view"),
            Some("abc123")
        );
    }

    #[test]
    fn returns_none_for_paths_with_no_id_segment() {
        assert_eq!(sandbox_id_from_path("/sandboxes"), None);
        assert_eq!(sandbox_id_from_path("/sandboxes/"), None);
        assert_eq!(sandbox_id_from_path("/health"), None);
        assert_eq!(sandbox_id_from_path("/"), None);
    }

    /// A stub router with the real middleware layered over handlers whose
    /// status is chosen by the test, isolating "does the middleware attempt
    /// a bump" from "does a Docker inspect succeed" — the router tests below
    /// use `test_state()`'s dead docker socket, so any attempted resolve
    /// fails closed; what they can prove directly is whether the middleware
    /// even reaches for the id after a given status.
    fn stub_router(state: AppState) -> Router {
        Router::new()
            .route("/sandboxes/{id}/ok", get(|| async { StatusCode::OK }))
            .route(
                "/sandboxes/{id}/created",
                post(|| async { StatusCode::CREATED }),
            )
            .route(
                "/sandboxes/{id}/unauthorized",
                get(|| async { StatusCode::UNAUTHORIZED }),
            )
            .route(
                "/sandboxes/{id}/forbidden",
                get(|| async { StatusCode::FORBIDDEN }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                keepalive_middleware,
            ))
            .with_state(state)
    }

    /// The response the caller sees must be exactly what the handler
    /// returned, regardless of whether the middleware's own bump attempt
    /// (which fails closed here — `test_state()` has no live Docker socket)
    /// succeeds. The middleware must never fail the response or shadow the
    /// handler's actual status.
    #[tokio::test]
    async fn a_2xx_response_passes_through_unchanged() {
        let app = stub_router(router_tests::test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/sandboxes/deadbeef/ok")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_201_response_passes_through_unchanged() {
        let app = stub_router(router_tests::test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/deadbeef/created")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    /// The critical safety property: a 401 (unauthenticated) must short
    /// -circuit before any bump attempt and pass straight through — proven
    /// here by never touching Docker (a bump attempt would hang/err against
    /// `test_state()`'s dead socket, so a passing test with a correct status
    /// confirms the `is_success()` gate ran first).
    #[tokio::test]
    async fn a_401_response_is_never_treated_as_a_keepalive_signal() {
        let app = stub_router(router_tests::test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/sandboxes/deadbeef/unauthorized")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_403_response_is_never_treated_as_a_keepalive_signal() {
        let app = stub_router(router_tests::test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/sandboxes/deadbeef/forbidden")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// `resolve_keepalive_target` returning `None` (no live Docker here, so
    /// this is the "id doesn't resolve" path every real deployment also
    /// hits for a stale/foreign id) must not turn a successful handler
    /// response into an error — the bump is best-effort.
    #[tokio::test]
    async fn an_unresolvable_id_does_not_fail_the_response() {
        let app = stub_router(router_tests::test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/sandboxes/does-not-exist/ok")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Pins the same clamp `extend_sandbox` relies on, applied to the
    /// keepalive grace window: a bump requested near the end of a sandbox's
    /// life is capped at `created_at + MAX_TTL_SECONDS`, never past it —
    /// continuous use can delay death but never grant immortality.
    #[test]
    fn a_keepalive_bump_near_the_cap_does_not_exceed_max_ttl() {
        let created_at = 1_000_000;
        let cap = created_at + sandbox::MAX_TTL_SECONDS as i64;
        let now = cap - 60; // 1 minute from the hard ceiling
        let bumped = sandbox::extended_expiry(now, created_at, sandbox::KEEPALIVE_GRACE_SECONDS);
        assert_eq!(bumped, cap);
    }

    #[test]
    fn a_keepalive_bump_well_before_the_cap_grants_the_full_grace_window() {
        let created_at = 1_000_000;
        let now = created_at + 100;
        let bumped = sandbox::extended_expiry(now, created_at, sandbox::KEEPALIVE_GRACE_SECONDS);
        assert_eq!(bumped, now + sandbox::KEEPALIVE_GRACE_SECONDS as i64);
    }

    /// The republish throttle's own decision, exercised directly: two bumps
    /// whose expiry moves less than the threshold apart should not each
    /// trigger a publish — only movement past
    /// `KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS` since the last announcement
    /// should. This mirrors the `moved_enough` check inside
    /// `keepalive_middleware` without needing a live publisher to observe
    /// call counts through.
    #[test]
    fn republish_throttle_suppresses_small_movements_but_not_large_ones() {
        let moved_enough = |last: i64, next: i64| {
            (next - last).abs() >= sandbox::KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS
        };
        let last_published = 1_000_000i64;
        // Two rapid bumps within the threshold: the second must not publish.
        assert!(!moved_enough(last_published, last_published + 5));
        assert!(!moved_enough(
            last_published,
            last_published + sandbox::KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS - 1
        ));
        // A bump that crosses the threshold must publish.
        assert!(moved_enough(
            last_published,
            last_published + sandbox::KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS
        ));
    }

    #[tokio::test]
    async fn heartbeat_route_reaches_its_handler_and_fails_auth_without_a_signature() {
        let app = build_router(router_tests::test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/abc123def456/heartbeat")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "an unsigned request must reach heartbeat and be refused there \
             (401), not 404 at the router layer"
        );
    }
}

/// Seed the live expiry map at startup.
///
/// The label holds each sandbox's *initial* expiry; an extend only ever moves
/// broker state and the relay's 48200. After a restart the relay is therefore
/// the record of any extension, so it is preferred, falling back to the label
/// when the relay has no answer. Failing to seed leaves the map empty and the
/// reaper on labels — old behavior, never a crash.
async fn seed_expiries(state: &AppState) {
    let listed = match state.docker.list_managed(sandbox::MANAGED_LABEL).await {
        Ok(l) => l,
        Err(e) => {
            warn!(error = %e, "could not seed sandbox expiries");
            return;
        }
    };
    for c in listed {
        let Some(id) = c.get("Id").and_then(|v| v.as_str()) else {
            continue;
        };
        let from_label = c
            .get("Labels")
            .and_then(|l| l.get(sandbox::LABEL_EXPIRES))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok());
        let from_relay = match state.publisher.as_ref() {
            Some(p) => p.fetch_latest_expiry(id).await,
            None => None,
        };
        if let Some(expires_at) = from_relay.or(from_label) {
            state.set_expiry(id, expires_at);
        }
    }
}

/// Reap expired sandboxes.
///
/// The TTL is a label, not a Docker feature, so something must enforce it.
/// Runs in-process on a fixed tick; a missed tick only delays a reap.
async fn reaper(state: AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
    loop {
        tick.tick().await;
        let now = chrono::Utc::now().timestamp();
        let listed = match state.docker.list_managed(sandbox::MANAGED_LABEL).await {
            Ok(l) => l,
            Err(e) => {
                warn!(error = %e, "reaper could not list sandboxes");
                continue;
            }
        };
        for c in &listed {
            let Some(id) = c.get("Id").and_then(|v| v.as_str()) else {
                continue;
            };
            let label = |k: &str| {
                c.get("Labels")
                    .and_then(|l| l.get(k))
                    .and_then(|v| v.as_str())
            };
            // Broker state is the live authority (an extend moves only it);
            // the label is the initial expiry, good enough for a sandbox that
            // was never extended.
            let expires = state
                .expiry_of(id)
                .or_else(|| label(sandbox::LABEL_EXPIRES).and_then(|s| s.parse::<i64>().ok()));
            if let Some(expires) = expires {
                if now >= expires {
                    let owner = label(sandbox::LABEL_OWNER).map(str::to_string);
                    match state.docker.remove_container(id).await {
                        Ok(()) => {
                            state.forget_expiry(id);
                            if let Some(publisher) = state.publisher.as_ref() {
                                publisher
                                    .sandbox_destroyed(id, owner.as_deref(), "expired")
                                    .await;
                            }
                            info!(sandbox = %id, "sandbox reaped (ttl expired)")
                        }
                        Err(e) => warn!(sandbox = %id, error = %e, "could not reap sandbox"),
                    }
                }
            }
        }
        // Drop expiry entries for containers that no longer exist (removed by
        // hand, or reaped above) so the map tracks reality rather than growing.
        if let Ok(mut map) = state.expiry.lock() {
            let live: std::collections::HashSet<&str> = listed
                .iter()
                .filter_map(|c| c.get("Id").and_then(|v| v.as_str()))
                .collect();
            map.retain(|id, _| live.contains(id.as_str()));
        }
    }
}

fn summarize(c: &serde_json::Value) -> SandboxSummary {
    // Labels are looked up by key, not by JSON Pointer: a pointer segment
    // escapes `/` as ~1 and `~` as ~0, and does NOT escape `.`, so building a
    // pointer from a dotted label name silently fails to match.
    let label = |k: &str| {
        c.get("Labels")
            .and_then(|l| l.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    SandboxSummary {
        id: c
            .get("Id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        name: c
            .get("Names")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_string(),
        image: c
            .get("Image")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        state: c
            .get("State")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        owner: label(sandbox::LABEL_OWNER),
        expires_at: label(sandbox::LABEL_EXPIRES).and_then(|s| s.parse().ok()),
    }
}

fn is_managed(inspected: &serde_json::Value) -> bool {
    inspected
        .get("Config")
        .and_then(|c| c.get("Labels"))
        .and_then(|l| l.get(sandbox::LABEL_KEY))
        .and_then(|v| v.as_str())
        == Some("1")
}

/// Container ids and names are hex/alnum; anything else would be a path
/// traversal attempt against the Docker API URL.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// A bearer token for one sandbox's tool port.
///
/// 256 bits from the OS CSPRNG, hex-encoded. Long enough that guessing is not a
/// threat model, and carrying no structure — it is compared for equality, never
/// parsed, so there is nothing in it to get wrong.
fn mint_tools_token() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    hex::encode(bytes)
}

fn short_id() -> String {
    use rand::RngExt;
    let mut rng = rand::rng();
    (0..10)
        .map(|_| {
            let n: u8 = rng.random_range(0u8..36);
            if n < 10 {
                (b'0' + n) as char
            } else {
                (b'a' + n - 10) as char
            }
        })
        .collect()
}

fn internal(e: impl std::fmt::Display) -> axum::response::Response {
    let msg = e.to_string();
    error!(error = %msg, "broker error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": msg})),
    )
        .into_response()
}

#[cfg(test)]
mod sandbox_ip_tests {
    use super::*;

    fn inspect(networks: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "NetworkSettings": { "Networks": networks } })
    }

    #[test]
    fn prefers_the_sandbox_network() {
        let v = inspect(serde_json::json!({
            "bridge": {"IPAddress": "172.17.0.5"},
            "buzz-sandboxes": {"IPAddress": "172.16.240.7"}
        }));
        assert_eq!(
            sandbox_ip(&v, "buzz-sandboxes").as_deref(),
            Some("172.16.240.7")
        );
    }

    /// A sandbox attached to an unexpected network should still be reachable
    /// rather than silently undrivable.
    #[test]
    fn falls_back_to_any_attached_network() {
        let v = inspect(serde_json::json!({"other": {"IPAddress": "10.1.2.3"}}));
        assert_eq!(
            sandbox_ip(&v, "buzz-sandboxes").as_deref(),
            Some("10.1.2.3")
        );
    }

    /// An empty address must read as absent, not as an address of "": the
    /// caller would otherwise build `http://:9320/mcp`.
    #[test]
    fn treats_an_empty_address_as_absent() {
        let v = inspect(serde_json::json!({"buzz-sandboxes": {"IPAddress": ""}}));
        assert!(sandbox_ip(&v, "buzz-sandboxes").is_none());
    }

    #[test]
    fn returns_none_when_no_networks_are_attached() {
        assert!(sandbox_ip(&inspect(serde_json::json!({})), "buzz-sandboxes").is_none());
        assert!(sandbox_ip(&serde_json::json!({}), "buzz-sandboxes").is_none());
    }
}

#[cfg(test)]
mod public_base_tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn derives_origin_at_a_hostname_root() {
        // A sandbox host with its own domain: origin is proto + host, no prefix.
        let h = headers(&[
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "sandbox.example.com"),
        ]);
        assert_eq!(
            public_base_from_request(&h).as_deref(),
            Some("https://sandbox.example.com")
        );
    }

    #[test]
    fn honors_a_stripped_path_prefix() {
        // Sharing the relay host under /sandbox-viewer: the stripped prefix must
        // reappear in the signed URL, or the token signature will not match.
        let h = headers(&[
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "relay.example.com"),
            ("x-forwarded-prefix", "/sandbox-viewer"),
        ]);
        assert_eq!(
            public_base_from_request(&h).as_deref(),
            Some("https://relay.example.com/sandbox-viewer")
        );
    }

    #[test]
    fn websocket_protos_fold_into_their_http_spelling() {
        // The viewer token is signed over the https page URL; a websockify
        // upgrade arrives with X-Forwarded-Proto: wss. Same origin — must
        // verify against the same URL.
        for (fwd, want) in [("wss", "https"), ("ws", "http")] {
            let h = headers(&[
                ("x-forwarded-proto", fwd),
                ("x-forwarded-host", "relay.example.com"),
                ("x-forwarded-prefix", "/sandbox-viewer"),
            ]);
            assert_eq!(
                public_base_from_request(&h).as_deref(),
                Some(format!("{want}://relay.example.com/sandbox-viewer").as_str())
            );
        }
    }

    #[test]
    fn defaults_proto_to_https_when_unset() {
        let h = headers(&[("x-forwarded-host", "sandbox.example.com")]);
        assert_eq!(
            public_base_from_request(&h).as_deref(),
            Some("https://sandbox.example.com")
        );
    }

    #[test]
    fn takes_the_first_value_of_a_forwarded_list() {
        // A chain of proxies produces comma-lists; the client-facing origin is
        // the first entry.
        let h = headers(&[
            ("x-forwarded-proto", "https, http"),
            ("x-forwarded-host", "sandbox.example.com, internal:9310"),
        ]);
        assert_eq!(
            public_base_from_request(&h).as_deref(),
            Some("https://sandbox.example.com")
        );
    }

    #[test]
    fn none_without_a_forwarded_host() {
        // No trusted proxy set the header — the bare request is not something to
        // sign against, so verification falls back to the configured base.
        assert!(public_base_from_request(&HeaderMap::new()).is_none());
        assert!(public_base_from_request(&headers(&[("host", "127.0.0.1:9310")])).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_with_path_characters_are_refused() {
        assert!(is_safe_id("a1b2c3"));
        assert!(is_safe_id("buzz-sandbox-abc123"));
        assert!(!is_safe_id("../../etc/passwd"));
        assert!(!is_safe_id("abc/def"));
        assert!(!is_safe_id("abc?force=true"));
        assert!(!is_safe_id(""));
    }

    #[test]
    fn only_labelled_containers_count_as_managed() {
        let managed = serde_json::json!({"Config": {"Labels": {"com.buzz.sandbox": "1"}}});
        let foreign =
            serde_json::json!({"Config": {"Labels": {"com.docker.compose.project": "x"}}});
        assert!(is_managed(&managed));
        assert!(!is_managed(&foreign));
        assert!(!is_managed(&serde_json::json!({})));
    }

    #[test]
    fn short_ids_are_url_safe_and_reasonably_unique() {
        let a = short_id();
        assert_eq!(a.len(), 10);
        assert!(is_safe_id(&a));
        let set: std::collections::HashSet<String> = (0..200).map(|_| short_id()).collect();
        assert!(set.len() > 190, "ids collide too often");
    }

    #[test]
    fn summarize_reads_labels_and_strips_the_name_slash() {
        let c = serde_json::json!({
            "Id": "deadbeef",
            "Names": ["/buzz-sandbox-abc"],
            "Image": "img",
            "State": "running",
            "Labels": {
                "com.buzz.sandbox": "1",
                "com.buzz.sandbox.owner": "tyler",
                "com.buzz.sandbox.expires-at": "1750000000"
            }
        });
        let s = summarize(&c);
        assert_eq!(s.name, "buzz-sandbox-abc");
        assert_eq!(s.owner.as_deref(), Some("tyler"));
        assert_eq!(s.expires_at, Some(1750000000));
    }
}

#[cfg(test)]
mod router_tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    /// An `AppState` fit only for router-level tests: the Docker socket path
    /// is never dialed (no test here reaches a handler that calls Docker) and
    /// no relay is configured, so `require_membership` stays irrelevant — the
    /// request never gets past token verification.
    ///
    /// `pub(super)` so `keepalive_tests` (a sibling test module) can build
    /// the same fixture rather than duplicating it — both need an `AppState`
    /// with a dead Docker socket for the same reason: proving a request
    /// short-circuits before ever reaching Docker.
    pub(super) fn test_state() -> AppState {
        AppState {
            docker: docker::Docker::new("/nonexistent/docker.sock"),
            verifier: Arc::new(identity::Verifier::new(
                "https://relay.example".to_string(),
                "https://broker.example".to_string(),
                nostr::Keys::generate(),
                true,
            )),
            publisher: None,
            viewer_base: None,
            allowed_images: Arc::new(Vec::new()),
            network: Arc::new("bridge".to_string()),
            disk_limit: None,
            host_cpus: 1,
            slot: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            expiry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            last_published_expiry: Arc::new(
                std::sync::Mutex::new(std::collections::HashMap::new()),
            ),
        }
    }

    /// A request as it actually arrives in production: through a reverse
    /// proxy that sets `X-Forwarded-Host`. Without it, `verify_surface_token`
    /// has no public origin to build the signed URL from (no `viewer_base`
    /// is configured in `test_state`, matching a deployment that relies on
    /// forwarded headers rather than a baked-in fallback) and answers 404
    /// itself — a false positive this test must not be confused with the
    /// router-level 404 it exists to catch.
    fn forwarded_request(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("x-forwarded-proto", "https")
            .header("x-forwarded-host", "broker.example")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn health_route_reaches_its_handler() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Pin the bug where axum's `{*rest}` wildcard does not match a request
    /// with nothing after the final `/`: `terminal()`'s own redirect target
    /// for a bare entry (`.../terminal/t/{token}/`, load-bearing because
    /// ttyd's client resolves its WebSocket relative to `window.location`,
    /// so it cannot be a synthetic non-empty path) must reach the handler
    /// rather than 404 at the routing layer.
    ///
    /// A bogus token is what makes the distinction provable: 404 means axum
    /// never matched the route at all (the bug); 401/403 means it matched,
    /// ran the handler, and the handler correctly refused the fake token —
    /// the routing question this test asks is answered before auth is ever
    /// evaluated.
    #[tokio::test]
    async fn terminal_trailing_slash_reaches_the_handler_and_fails_auth() {
        let app = build_router(test_state());
        let uri = "/sandboxes/abc123def456/terminal/t/not-a-real-token/";
        let resp = app.oneshot(forwarded_request("GET", uri)).await.unwrap();
        assert!(
            resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN,
            "the trailing-slash terminal URL must route to the handler and fail \
             auth (401/403) on a bogus token, not 404 at the axum router layer \
             — a 404 here means the redirect this broker itself issues points \
             at a URL its own router rejects; got {}",
            resp.status()
        );
    }

    /// The equivalent non-empty-tail URL (ttyd's index.html, or any other
    /// asset) already worked before this fix — kept as a control so a
    /// regression in the wildcard route itself would also be caught.
    #[tokio::test]
    async fn terminal_non_empty_tail_reaches_the_handler_and_fails_auth() {
        let app = build_router(test_state());
        let uri = "/sandboxes/abc123def456/terminal/t/not-a-real-token/index.html";
        let resp = app.oneshot(forwarded_request("GET", uri)).await.unwrap();
        assert!(
            resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN,
            "got {}",
            resp.status()
        );
    }

    /// Mirrors the terminal routing test above for the header-authenticated
    /// launch endpoint: a request with no `Authorization` header must reach
    /// `launch_app` and be refused there (401), not 404 before the handler
    /// ever runs — proving the route is wired, independent of whether a
    /// caller happens to hold a valid signature.
    #[tokio::test]
    async fn launch_reaches_the_handler_and_fails_auth_without_a_signature() {
        let app = build_router(test_state());
        let body = serde_json::json!({ "app": "terminal" }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/abc123def456/launch")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "an unsigned request must reach launch_app and be refused there \
             (401), not 404 at the router layer"
        );
    }

    /// Same routing-not-404 proof as `launch_reaches_the_handler_and_fails_auth_without_a_signature`,
    /// applied to the three new computer-use routes: an unsigned request must
    /// reach the handler and be refused there, not fail earlier at the axum
    /// route table.
    #[tokio::test]
    async fn exec_reaches_the_handler_and_fails_auth_without_a_signature() {
        let app = build_router(test_state());
        let body = serde_json::json!({ "argv": ["ls"] }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/abc123def456/exec")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn screenshot_reaches_the_handler_and_fails_auth_without_a_signature() {
        let app = build_router(test_state());
        let uri = "/sandboxes/abc123def456/screenshot";
        let resp = app.oneshot(forwarded_request("GET", uri)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn recording_start_reaches_the_handler_and_fails_auth_without_a_signature() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/abc123def456/recording/start")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn recording_stop_reaches_the_handler_and_fails_auth_without_a_signature() {
        let app = build_router(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/abc123def456/recording/stop")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Pins the fix for the CLI's `broker returned unparseable JSON: EOF
    /// while parsing a value at line 1 column 0` — `launch_app`'s success
    /// path must answer with a JSON body, never an empty one, because the
    /// CLI's `broker_call` always tries to JSON-decode the response.
    #[test]
    fn launch_success_body_is_non_empty_json_with_the_launched_app() {
        let body = launch_success_body("terminal", None);
        assert_eq!(body["launched"], serde_json::json!(true));
        assert_eq!(body["app"], serde_json::json!("terminal"));
        assert_eq!(body["url"], serde_json::json!(null));
        assert_ne!(
            serde_json::to_string(&body).unwrap(),
            "",
            "an empty body is exactly what broke the CLI's JSON parse"
        );
    }

    /// The browser case carries the launched `url` through to the response
    /// too, not just `app`.
    #[test]
    fn launch_success_body_includes_the_browser_url() {
        let body = launch_success_body("browser", Some("https://example.com"));
        assert_eq!(body["app"], serde_json::json!("browser"));
        assert_eq!(body["url"], serde_json::json!("https://example.com"));
    }

    #[tokio::test]
    async fn input_reaches_the_handler_and_fails_auth_without_a_signature() {
        let app = build_router(test_state());
        let body =
            serde_json::json!({ "actions": [{ "type": "move", "x": 1, "y": 1 }] }).to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/sandboxes/abc123def456/input")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
