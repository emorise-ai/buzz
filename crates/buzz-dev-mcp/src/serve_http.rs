//! Serving the tool surface over HTTP, for agents whose brain runs elsewhere.
//!
//! The default transport is stdio: the agent spawns this process as a child and
//! speaks MCP over the pipe, so the tools are reachable only by their parent and
//! authentication is the process boundary itself.
//!
//! That stops working once the reasoning model runs on a different machine from
//! the tools it drives — a sandbox on a shared host, with the agent (and its LLM
//! credential) staying on the operator's side. Then the tools have to be
//! reachable over the network, and the process boundary no longer protects them.
//!
//! **This endpoint is remote code execution by design.** `shell` runs arbitrary
//! commands; `str_replace` writes arbitrary files. Anyone who can call it owns
//! the sandbox. Two things keep that honest:
//!
//! * **Every request is authenticated by Buzz identity (NIP-98)**, the same
//!   scheme the relay's `/query` and the sandbox broker already use. There is no
//!   bearer token to distribute, rotate, or leak — the caller proves possession
//!   of a key.
//! * **Exactly one pubkey is authorized**, given at startup. A sandbox serves
//!   its own agent and nobody else, so this is an equality check rather than a
//!   membership list. There is no "open" mode: without `--owner` the server
//!   refuses to start rather than listening unauthenticated.
//!
//! Replay is bounded by the same 60s window the broker uses. The signature
//! covers the method, path, and body — which tool runs, with which arguments —
//! so those cannot be altered. The origin is verified only when an operator
//! pins one (`BUZZ_DEV_MCP_PUBLIC_URL`); see [`AuthConfig::allowed_origins`]
//! for why a sandbox generally cannot know its own address.

use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};

/// Wall-clock skew tolerated on a NIP-98 event, matching the broker and the
/// relay so a request one accepts is not refused here over a few seconds.
const MAX_AGE: Duration = Duration::from_secs(60);

/// Everything the auth layer needs, shared with each request.
#[derive(Clone)]
pub struct AuthConfig {
    /// The single pubkey permitted to drive these tools, hex-encoded.
    owner_pubkey: String,
    /// Origins this server accepts a signature for, lowercased and without a
    /// trailing slash. Empty means "any origin", the sandbox default.
    ///
    /// A sandbox cannot know the address its agent will use: it is created
    /// before it has an IP, may be reached by container IP, hostname, or
    /// through a tunnel, and each would produce a different signed URL. Pinning
    /// one would reject every request that arrived by another route, and the
    /// failure would look like a broken signature.
    ///
    /// Leaving it open is safe here because the origin is not what authorizes
    /// the call. The signature still covers the **method, path, and body** —
    /// which tool runs, with which arguments — and the key must still be the
    /// owner's. An attacker who could replay a captured header against a
    /// different host would need that header, and holding it already means
    /// holding a valid signed request for this exact body.
    ///
    /// An operator who does front this with a fixed origin can still pin it
    /// via `BUZZ_DEV_MCP_PUBLIC_URL`, which narrows the check.
    allowed_origins: Vec<String>,
}

impl AuthConfig {
    /// `public_url` pins the accepted origin; `None` accepts any.
    pub fn new(owner_pubkey: String, public_url: Option<String>) -> Result<Self, String> {
        let owner = owner_pubkey.trim().to_ascii_lowercase();
        // Parse rather than merely length-check: a malformed key here would
        // fail every comparison at runtime and look like a signing bug.
        nostr::PublicKey::parse(&owner)
            .map_err(|e| format!("owner is not a valid public key: {e}"))?;
        Ok(Self {
            owner_pubkey: owner,
            allowed_origins: public_url
                .map(|u| vec![u.trim().trim_end_matches('/').to_ascii_lowercase()])
                .unwrap_or_default(),
        })
    }

    /// Verify one request. Returns the caller's pubkey on success.
    ///
    /// Verification is entirely local — a signature check over the method, URL,
    /// and body — so an unauthenticated caller never causes a network call.
    fn verify(
        &self,
        headers: &HeaderMap,
        method: &str,
        path: &str,
        body: &[u8],
        origin: &str,
    ) -> Result<String, String> {
        if !self.allowed_origins.is_empty()
            && !self
                .allowed_origins
                .iter()
                .any(|o| o == &origin.to_ascii_lowercase())
        {
            return Err(format!(
                "request origin {origin} is not one this server accepts signatures for"
            ));
        }
        let raw = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Nostr "))
            .ok_or_else(|| {
                "missing NIP-98 auth: sign the request with the agent's Buzz \
                 identity (Authorization: Nostr <base64 event>)"
                    .to_string()
            })?;

        let json = {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw.trim())
                .map_err(|_| "auth header is not valid base64".to_string())?;
            String::from_utf8(bytes).map_err(|_| "auth header is not valid UTF-8".to_string())?
        };

        let url = format!("{origin}{path}");
        let pubkey = buzz_auth::verify_nip98_event(&json, &url, method, Some(body))
            .map_err(|e| format!("NIP-98 verification failed: {e}"))?;

        // The signature proves authorship, not freshness; without an age bound a
        // captured header would be replayable for as long as the sandbox lives.
        let event: serde_json::Value =
            serde_json::from_str(&json).map_err(|_| "auth event is not JSON".to_string())?;
        if let Some(created_at) = event.get("created_at").and_then(|v| v.as_i64()) {
            let now = chrono::Utc::now().timestamp();
            if (now - created_at).unsigned_abs() > MAX_AGE.as_secs() {
                return Err("auth event is too old or too far in the future".to_string());
            }
        }

        // One sandbox serves one agent. A valid signature from any other key is
        // still a refusal: authentication is not authorization.
        let hex = pubkey.to_hex();
        if hex != self.owner_pubkey {
            return Err("this sandbox is not owned by that identity".to_string());
        }
        Ok(hex)
    }
}

/// Paths an MCP client probes to discover an OAuth authorization server.
///
/// Listed explicitly rather than matched by prefix so that a tool path can
/// never be mistaken for discovery and served without authentication.
fn is_oauth_discovery(path: &str) -> bool {
    matches!(
        path,
        "/.well-known/oauth-authorization-server"
            | "/.well-known/oauth-protected-resource"
            | "/.well-known/oauth-protected-resource/mcp"
            | "/.well-known/openid-configuration"
            | "/register"
    )
}

/// Axum middleware rejecting anything that fails `AuthConfig::verify`.
///
/// Runs before the MCP service sees the request, so an unauthorized caller
/// never reaches a tool.
pub async fn require_owner(
    State(auth): State<AuthConfig>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let (parts, body) = request.into_parts();
    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();

    // An MCP client that meets a 401 assumes OAuth and goes looking for an
    // authorization server. Claude Code probes these five paths, and answering
    // any of them with a 401 reads as "the OAuth endpoint needs auth", which
    // sends it round the same loop; it then gives up and silently falls back to
    // local tools, so the sandbox is quietly unused rather than visibly broken.
    //
    // 404 is the honest answer: this server has no OAuth. It ends discovery in
    // one round trip and leaves the NIP-98 challenge below as the only route to
    // authenticate.
    if is_oauth_discovery(&path) {
        tracing::debug!(%path, "declining OAuth discovery: this server authenticates with NIP-98");
        return Err((
            StatusCode::NOT_FOUND,
            "this server does not use OAuth; sign requests with NIP-98 \
             (Authorization: Nostr <base64 event>)"
                .to_string(),
        ));
    }

    // The origin the caller signed, reconstructed from the request itself. The
    // scheme is not observable here (TLS, if any, terminates upstream), so it
    // is taken from `X-Forwarded-Proto` when a proxy states it and assumed
    // plain HTTP otherwise — which is what a direct sandbox connection is.
    let host = parts
        .headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| parts.uri.host())
        .unwrap_or_default()
        .to_string();
    let scheme = parts
        .headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    let origin = format!("{scheme}://{host}");

    // The body is part of what NIP-98 signs, so it must be buffered before
    // verification and put back afterwards.
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("could not read body: {e}")))?;

    if let Err(e) = auth.verify(&parts.headers, &method, &path, &bytes, &origin) {
        tracing::warn!(error = %e, %method, %path, "rejected an unauthenticated tool call");
        return Err((StatusCode::UNAUTHORIZED, e));
    }

    Ok(next
        .run(axum::extract::Request::from_parts(
            parts,
            axum::body::Body::from(bytes),
        ))
        .await)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "http://127.0.0.1:9320";

    fn keys() -> nostr::Keys {
        nostr::Keys::generate()
    }

    fn signed_header(keys: &nostr::Keys, method: &str, url: &str, body: &[u8]) -> String {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let mut tags = vec![
            nostr::Tag::parse(["u", url]).expect("u tag"),
            nostr::Tag::parse(["method", method]).expect("method tag"),
        ];
        if !body.is_empty() {
            let hash = hex::encode(Sha256::digest(body));
            tags.push(nostr::Tag::parse(["payload", &hash]).expect("payload tag"));
        }
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign");
        let json = serde_json::to_string(&event).expect("serialize");
        format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(json)
        )
    }

    fn headers_with(auth: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            auth.parse().expect("header value"),
        );
        h
    }

    #[test]
    fn rejects_a_malformed_owner_key() {
        assert!(AuthConfig::new("not-a-key".into(), None).is_err());
    }

    #[test]
    fn accepts_a_request_signed_by_the_owner() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), None).expect("config");
        let body = br#"{"jsonrpc":"2.0"}"#;
        let auth = signed_header(&k, "POST", "http://127.0.0.1:9320/mcp", body);
        assert_eq!(
            cfg.verify(&headers_with(&auth), "POST", "/mcp", body, ORIGIN)
                .expect("verify"),
            k.public_key().to_hex()
        );
    }

    /// The whole point of the endpoint: a valid signature from the wrong key is
    /// still refused. Authentication is not authorization.
    #[test]
    fn rejects_a_valid_signature_from_another_identity() {
        let owner = keys();
        let intruder = keys();
        let cfg = AuthConfig::new(owner.public_key().to_hex(), None).expect("config");
        let body = br#"{"jsonrpc":"2.0"}"#;
        let auth = signed_header(&intruder, "POST", "http://127.0.0.1:9320/mcp", body);
        let err = cfg
            .verify(&headers_with(&auth), "POST", "/mcp", body, ORIGIN)
            .expect_err("must refuse a non-owner");
        assert!(err.contains("not owned"), "unexpected error: {err}");
    }

    /// The sandbox default: reached by container IP, hostname, or a tunnel,
    /// each producing a different signed URL. All must verify, because the
    /// signature's authority is the method, path, body, and key — not the host.
    #[test]
    fn accepts_any_origin_when_none_is_pinned() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), None).expect("config");
        for origin in [
            "http://172.16.240.2:9320",
            "http://sandbox-abc:9320",
            "http://127.0.0.1:19320",
        ] {
            let auth = signed_header(&k, "POST", &format!("{origin}/mcp"), b"");
            assert!(
                cfg.verify(&headers_with(&auth), "POST", "/mcp", b"", origin)
                    .is_ok(),
                "should accept {origin}"
            );
        }
    }

    /// An operator fronting the server with a fixed origin can narrow the
    /// check, and then a signature for another host is refused.
    #[test]
    fn rejects_a_foreign_origin_when_one_is_pinned() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), Some(ORIGIN.into())).expect("config");

        let ok = signed_header(&k, "POST", &format!("{ORIGIN}/mcp"), b"");
        assert!(cfg
            .verify(&headers_with(&ok), "POST", "/mcp", b"", ORIGIN)
            .is_ok());

        let other = "http://evil.example";
        let bad = signed_header(&k, "POST", &format!("{other}/mcp"), b"");
        let err = cfg
            .verify(&headers_with(&bad), "POST", "/mcp", b"", other)
            .expect_err("must refuse a foreign origin");
        assert!(err.contains("origin"), "unexpected error: {err}");
    }

    /// A pinned origin must not be defeated by case or a trailing slash.
    #[test]
    fn pinned_origin_comparison_is_normalized() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), Some("HTTP://Host:9320/".into()))
            .expect("cfg");
        let origin = "http://host:9320";
        let auth = signed_header(&k, "POST", &format!("{origin}/mcp"), b"");
        assert!(cfg
            .verify(&headers_with(&auth), "POST", "/mcp", b"", origin)
            .is_ok());
    }

    /// The bug this guard exists for: answering discovery with 401 made Claude
    /// Code loop, give up, and silently use local tools instead of the sandbox.
    #[test]
    fn declines_every_oauth_discovery_path() {
        for p in [
            "/.well-known/oauth-authorization-server",
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/openid-configuration",
            "/register",
        ] {
            assert!(is_oauth_discovery(p), "{p} must end discovery");
        }
    }

    /// Discovery must never be a way to reach a tool unauthenticated.
    #[test]
    fn tool_paths_are_not_treated_as_discovery() {
        for p in ["/mcp", "/mcp/tools", "/", "/.well-known/../mcp"] {
            assert!(!is_oauth_discovery(p), "{p} must still require auth");
        }
    }

    #[test]
    fn rejects_a_missing_header() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), None).expect("config");
        assert!(cfg
            .verify(&HeaderMap::new(), "POST", "/mcp", b"", ORIGIN)
            .is_err());
    }

    /// A signature captured from one path must not authorize another — the URL
    /// is part of what is signed.
    #[test]
    fn rejects_a_signature_bound_to_a_different_path() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), None).expect("config");
        let auth = signed_header(&k, "POST", "http://127.0.0.1:9320/mcp", b"");
        assert!(cfg
            .verify(&headers_with(&auth), "POST", "/other", b"", ORIGIN)
            .is_err());
    }

    /// Body tampering must invalidate the signature, or an attacker could swap
    /// the JSON-RPC payload for a different tool call.
    #[test]
    fn rejects_a_tampered_body() {
        let k = keys();
        let cfg = AuthConfig::new(k.public_key().to_hex(), None).expect("config");
        let auth = signed_header(&k, "POST", "http://127.0.0.1:9320/mcp", b"original");
        assert!(cfg
            .verify(&headers_with(&auth), "POST", "/mcp", b"tampered", ORIGIN)
            .is_err());
    }
}
