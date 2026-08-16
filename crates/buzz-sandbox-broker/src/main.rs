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
//! default and authenticates every mutating request with a bearer token.

mod docker;
mod events;
mod identity;
mod sandbox;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use sandbox::{CreateRequest, Limits, SandboxSummary};
use std::sync::Arc;
use tracing::{error, info, warn};

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
    /// Public base URL a human uses to open a sandbox's live desktop. Absent
    /// when no viewer is exposed, in which case events carry no viewer link
    /// rather than an address that would not resolve.
    viewer_base: Option<Arc<String>>,
    allowed_images: Arc<Vec<String>>,
    network: Arc<String>,
    /// Writable-layer cap (e.g. "30G"). None on filesystems that cannot
    /// enforce one — see `container_spec`.
    disk_limit: Option<Arc<str>>,
    host_cpus: usize,
    slot: Arc<std::sync::atomic::AtomicUsize>,
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
    // The exact origin clients call, because NIP-98 signs the full URL and a
    // mismatch rejects every otherwise-valid request.
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
    };

    tokio::spawn(reaper(state.clone()));

    let app = Router::new()
        .route("/health", get(health))
        .route("/sandboxes", get(list_sandboxes).post(create_sandbox))
        .route("/sandboxes/{id}", get(get_sandbox).delete(delete_sandbox))
        .route("/sandboxes/{id}/stop", post(delete_sandbox))
        .with_state(state);

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
    match state.docker.list_managed(sandbox::MANAGED_LABEL).await {
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
        }
        Err(e) => return internal(e),
    }

    let limits = Limits::resolve(&req);
    let expires_at = chrono::Utc::now().timestamp() + limits.ttl_seconds as i64;
    let slot = state
        .slot
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cpuset = sandbox::assign_cpuset(state.host_cpus, limits.cpus, slot);
    let name = format!("buzz-sandbox-{}", short_id());

    let spec = sandbox::container_spec(sandbox::SpecInputs {
        image: &req.image,
        limits,
        env: &req.env,
        owner: req.owner.as_deref(),
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
        })),
    )
        .into_response()
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
            let out: Vec<SandboxSummary> = list.iter().map(summarize).collect();
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
                    "expires_at": v
                        .get("Config")
                        .and_then(|c| c.get("Labels"))
                        .and_then(|l| l.get(sandbox::LABEL_EXPIRES)),
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
    match state.docker.inspect_container(&id).await {
        Ok(v) if !is_managed(&v) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "no such sandbox"})),
            )
                .into_response()
        }
        Ok(_) => {}
        Err(e) if e.contains("404") || e.to_lowercase().contains("no such container") => {
            // Already gone is success: the caller's intent is satisfied.
            return (StatusCode::NO_CONTENT, ()).into_response();
        }
        Err(e) => return internal(e),
    }
    match state.docker.remove_container(&id).await {
        Ok(()) => {
            if let Some(publisher) = state.publisher.as_ref() {
                publisher.sandbox_destroyed(&id, None, "destroyed").await;
            }
            info!(sandbox = %id, "sandbox destroyed");
            (StatusCode::NO_CONTENT, ()).into_response()
        }
        Err(e) => internal(e),
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
        for c in listed {
            let Some(id) = c.get("Id").and_then(|v| v.as_str()) else {
                continue;
            };
            let expires = c
                .get("Labels")
                .and_then(|l| l.get(sandbox::LABEL_EXPIRES))
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<i64>().ok());
            if let Some(expires) = expires {
                if now >= expires {
                    match state.docker.remove_container(id).await {
                        Ok(()) => {
                            if let Some(publisher) = state.publisher.as_ref() {
                                publisher.sandbox_destroyed(id, None, "expired").await;
                            }
                            info!(sandbox = %id, "sandbox reaped (ttl expired)")
                        }
                        Err(e) => warn!(sandbox = %id, error = %e, "could not reap sandbox"),
                    }
                }
            }
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
