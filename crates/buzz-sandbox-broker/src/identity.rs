//! Buzz identity for the broker — NIP-98 instead of a shared bearer token.
//!
//! A shared token is a second credential system: it has to be distributed to
//! every launcher, it says nothing about *who* is calling, and rotating it
//! means touching every caller. Buzz already authenticates every participant
//! by a Nostr keypair, so the broker uses that instead — the same signed-HTTP
//! scheme (NIP-98) the relay's own `/query` and `/count` endpoints accept.
//!
//! Two consequences worth stating, because they are the point:
//!
//! * **The caller is identified, not merely admitted.** The verified pubkey
//!   becomes the sandbox's owner, so "whose sandbox is this?" has an answer
//!   that survives a restart and cannot be forged by whoever holds a token.
//! * **Authorization is delegated to the relay.** The broker does not keep its
//!   own list of who may run sandboxes; it asks the relay whether the caller is
//!   a member. One membership list, not two.

use std::time::Duration;

/// How long a verified membership answer stays good.
///
/// The relay is the authority, but asking it on every call would put the
/// broker's availability at the mercy of a network hop. A short cache keeps
/// revocation timely (a removed member loses access within a minute) without
/// making every sandbox operation depend on a live round trip.
const MEMBERSHIP_CACHE_TTL: Duration = Duration::from_secs(60);

/// Wall-clock skew tolerated on a NIP-98 event's timestamp, matching the
/// relay's own tolerance so a request the relay would accept is not refused
/// here for a clock difference of a few seconds.
const MAX_AGE: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Verifier {
    /// Relay base URL used both to check membership and to publish events.
    relay_url: String,
    /// The broker's own identity, used to sign its membership queries. The
    /// relay authenticates every `/query`, so an unsigned check would fail
    /// closed for *everyone* — the broker has to be a participant, not an
    /// anonymous caller.
    keys: nostr::Keys,
    /// Whether membership is required at all. On an open relay the relay
    /// itself admits any authenticated caller, so demanding a membership event
    /// here would be stricter than the authority the broker defers to — it
    /// would refuse people the relay accepts.
    require_membership: bool,
    /// The broker's own public origin, needed because NIP-98 signs the exact
    /// URL the client called — a mismatch here rejects every request.
    public_url: String,
    cache: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, (bool, std::time::Instant)>>,
    >,
}

impl Verifier {
    pub fn new(
        relay_url: String,
        public_url: String,
        keys: nostr::Keys,
        require_membership: bool,
    ) -> Self {
        Self {
            relay_url: relay_url.trim_end_matches('/').to_string(),
            public_url: public_url.trim_end_matches('/').to_string(),
            keys,
            require_membership,
            cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Verify a NIP-98 `Authorization: Nostr <base64>` header.
    ///
    /// Returns the caller's hex pubkey. Verification is entirely local — a
    /// signature check against the request's method, URL, and body — so an
    /// unauthenticated caller never causes a network call.
    pub fn verify(
        &self,
        headers: &axum::http::HeaderMap,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<String, String> {
        let raw = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Nostr "))
            .ok_or_else(|| {
                "missing NIP-98 auth: sign the request with your Buzz identity \
                 (Authorization: Nostr <base64 event>)"
                    .to_string()
            })?;

        let json = {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw.trim())
                .map_err(|_| "auth header is not valid base64".to_string())?;
            String::from_utf8(bytes).map_err(|_| "auth header is not valid UTF-8".to_string())?
        };

        let url = format!("{}{path}", self.public_url);
        let pubkey = buzz_auth::verify_nip98_event(&json, &url, method, Some(body))
            .map_err(|e| format!("NIP-98 verification failed: {e}"))?;

        // The signature proves authorship, not freshness; without an age bound
        // a captured header would be replayable forever.
        let event: serde_json::Value =
            serde_json::from_str(&json).map_err(|_| "auth event is not JSON".to_string())?;
        if let Some(created_at) = event.get("created_at").and_then(|v| v.as_i64()) {
            let now = chrono::Utc::now().timestamp();
            if (now - created_at).unsigned_abs() > MAX_AGE.as_secs() {
                return Err("auth event is too old or too far in the future".to_string());
            }
        }

        Ok(pubkey.to_hex())
    }

    /// Is this pubkey a member of the relay's community?
    ///
    /// The broker deliberately holds no membership list of its own. It asks the
    /// relay, which is the authority, and caches the answer briefly.
    ///
    /// Fails **closed**: if the relay cannot be reached, the answer is "no".
    /// An unreachable relay means an agent could not work anyway, so refusing
    /// costs nothing and prevents the broker becoming a way to run containers
    /// while the authority that gates it is down.
    pub async fn is_member(&self, pubkey_hex: &str) -> bool {
        // An open relay admits any authenticated caller. Requiring a
        // membership event here would make the broker stricter than the
        // authority it defers to, refusing people the relay itself accepts.
        if !self.require_membership {
            return true;
        }
        if let Some(cached) = self.cached(pubkey_hex) {
            return cached;
        }

        let allowed = self.ask_relay(pubkey_hex).await;
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(pubkey_hex.to_string(), (allowed, std::time::Instant::now()));
        }
        allowed
    }

    fn cached(&self, pubkey_hex: &str) -> Option<bool> {
        let cache = self.cache.lock().ok()?;
        let (allowed, at) = cache.get(pubkey_hex)?;
        (at.elapsed() < MEMBERSHIP_CACHE_TTL).then_some(*allowed)
    }

    /// Ask the relay whether this pubkey has a membership event.
    ///
    /// Uses the relay's public metadata rather than a privileged API: a
    /// kind:39002 membership event authored for this pubkey is what makes
    /// someone a member, and `POST /query` is the same door any client uses.
    async fn ask_relay(&self, pubkey_hex: &str) -> bool {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        let filter = serde_json::json!([{
            "kinds": [39002],
            "#d": [pubkey_hex],
            "limit": 1
        }]);
        let body = filter.to_string();
        let url = format!("{}/query", self.relay_url);
        // The relay authenticates /query like any other caller, so the broker
        // signs its own request. Without this the check always failed and the
        // broker refused everyone.
        let Some(auth) = self.sign_request("POST", &url, body.as_bytes()) else {
            return false;
        };
        match client
            .post(&url)
            .header(axum::http::header::AUTHORIZATION, format!("Nostr {auth}"))
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(v) => v.as_array().map(|a| !a.is_empty()).unwrap_or(false),
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }

    /// Build a NIP-98 `Authorization` value for the broker's own request.
    fn sign_request(&self, method: &str, url: &str, body: &[u8]) -> Option<String> {
        use base64::Engine;
        use sha2::{Digest, Sha256};

        let mut tags = vec![
            nostr::Tag::parse(["u", url]).ok()?,
            nostr::Tag::parse(["method", method]).ok()?,
        ];
        if !body.is_empty() {
            let hash = hex::encode(Sha256::digest(body));
            tags.push(nostr::Tag::parse(["payload", &hash]).ok()?);
        }
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .ok()?;
        Some(base64::engine::general_purpose::STANDARD.encode(serde_json::to_string(&event).ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn verifier() -> Verifier {
        Verifier::new(
            "https://relay.example/".into(),
            "https://broker.example/".into(),
            nostr::Keys::generate(),
            true,
        )
    }

    #[test]
    fn trailing_slashes_are_normalized_so_urls_match_exactly() {
        // NIP-98 signs the exact URL; a doubled slash would reject every
        // otherwise-valid request.
        let v = verifier();
        assert_eq!(v.relay_url, "https://relay.example");
        assert_eq!(v.public_url, "https://broker.example");
    }

    #[test]
    fn a_request_without_an_auth_header_is_refused() {
        let v = verifier();
        let err = v
            .verify(&HeaderMap::new(), "POST", "/sandboxes", b"{}")
            .unwrap_err();
        assert!(err.contains("missing NIP-98"), "got: {err}");
    }

    #[test]
    fn a_bearer_token_is_no_longer_accepted() {
        // The whole point of this module: the old shared-token path must not
        // still work, or the weaker credential remains a way in.
        let v = verifier();
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer 0123456789abcdef0123456789abcdef"),
        );
        assert!(v.verify(&h, "POST", "/sandboxes", b"{}").is_err());
    }

    #[test]
    fn malformed_auth_headers_are_refused_before_any_network_call() {
        let v = verifier();
        for bad in ["Nostr !!!not-base64!!!", "Nostr ", "Nostr bm90LWpzb24="] {
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::header::AUTHORIZATION,
                HeaderValue::from_str(bad).unwrap(),
            );
            assert!(
                v.verify(&h, "POST", "/sandboxes", b"{}").is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    /// An open relay admits any authenticated caller; the broker must not be
    /// stricter than the authority it defers to, or it refuses people the
    /// relay itself accepts.
    #[tokio::test]
    async fn an_open_relay_admits_any_authenticated_caller() {
        let v = Verifier::new(
            "https://relay.example".into(),
            "https://broker.example".into(),
            nostr::Keys::generate(),
            false,
        );
        // No network call is made, and the answer is yes.
        assert!(v.is_member("any-pubkey").await);
    }

    #[test]
    fn membership_cache_expires_rather_than_pinning_a_stale_answer() {
        let v = verifier();
        {
            let mut cache = v.cache.lock().unwrap();
            // An answer older than the TTL must not be served.
            cache.insert(
                "stale".into(),
                (
                    true,
                    std::time::Instant::now() - MEMBERSHIP_CACHE_TTL - Duration::from_secs(1),
                ),
            );
            cache.insert("fresh".into(), (true, std::time::Instant::now()));
        }
        assert_eq!(v.cached("stale"), None, "expired entries must be ignored");
        assert_eq!(v.cached("fresh"), Some(true));
        assert_eq!(v.cached("unknown"), None);
    }
}
