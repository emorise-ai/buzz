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
    /// The broker's advertised origins, needed because NIP-98 signs the exact
    /// URL the client called — a mismatch rejects every request. More than one
    /// because the broker is legitimately reachable at several addresses at
    /// once: the operator's loopback tunnel, its container DNS name on the
    /// sandbox network (how `buzz sandbox` inside a sandbox calls it), and a
    /// public reverse-proxy path. A caller signs over whichever it used; the
    /// broker accepts any *configured* origin — never one merely claimed by a
    /// forwarded header, which would let a request signed for some other
    /// broker be replayed against this one.
    public_urls: Vec<String>,
    cache: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, (bool, std::time::Instant)>>,
    >,
}

impl Verifier {
    fn auth_json(headers: &axum::http::HeaderMap) -> Result<String, String> {
        let raw = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Nostr "))
            .ok_or_else(|| {
                "missing NIP-98 auth: sign the request with your Buzz identity \
                 (Authorization: Nostr <base64 event>)"
                    .to_string()
            })?;
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.trim())
            .map_err(|_| "auth header is not valid base64".to_string())?;
        String::from_utf8(bytes).map_err(|_| "auth header is not valid UTF-8".to_string())
    }

    pub fn new(
        relay_url: String,
        public_url: String,
        keys: nostr::Keys,
        require_membership: bool,
    ) -> Self {
        Self {
            relay_url: relay_url.trim_end_matches('/').to_string(),
            // Comma-separated so one env var carries every address the broker
            // answers on.
            public_urls: public_url
                .split(',')
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty())
                .collect(),
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
        let json = Self::auth_json(headers)?;

        // The signature binds one exact URL; try each configured origin. All
        // checks are local, so the cost of a short list is nil, and the error
        // reported is the last mismatch — every candidate failed the same way.
        let mut pubkey = None;
        let mut last_err = String::from("no public URL configured");
        for base in &self.public_urls {
            let url = format!("{base}{path}");
            match buzz_auth::verify_nip98_event(&json, &url, method, Some(body)) {
                Ok(pk) => {
                    pubkey = Some(pk);
                    break;
                }
                Err(e) => last_err = format!("NIP-98 verification failed: {e}"),
            }
        }
        let Some(pubkey) = pubkey else {
            return Err(last_err);
        };

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

    /// Verify a request before its body is streamed and return the SHA-256
    /// digest the signed NIP-98 event commits to. The caller must compare this
    /// digest after consuming the bounded stream.
    pub fn verify_streaming_body(
        &self,
        headers: &axum::http::HeaderMap,
        method: &str,
        path: &str,
    ) -> Result<(String, [u8; 32]), String> {
        let json = Self::auth_json(headers)?;
        let mut pubkey = None;
        let mut last_err = String::from("no public URL configured");
        for base in &self.public_urls {
            let url = format!("{base}{path}");
            match buzz_auth::verify_nip98_event(&json, &url, method, None) {
                Ok(pk) => {
                    pubkey = Some(pk);
                    break;
                }
                Err(e) => last_err = format!("NIP-98 verification failed: {e}"),
            }
        }
        let Some(pubkey) = pubkey else {
            return Err(last_err);
        };
        let event: serde_json::Value =
            serde_json::from_str(&json).map_err(|_| "auth event is not JSON".to_string())?;
        let payload = event
            .get("tags")
            .and_then(|tags| tags.as_array())
            .and_then(|tags| {
                tags.iter().find_map(|tag| {
                    let tag = tag.as_array()?;
                    (tag.first()?.as_str()? == "payload")
                        .then(|| tag.get(1)?.as_str())
                        .flatten()
                })
            })
            .ok_or_else(|| "streaming upload auth is missing its payload hash".to_string())?;
        let decoded = hex::decode(payload)
            .map_err(|_| "streaming upload payload hash is not valid hex".to_string())?;
        let digest: [u8; 32] = decoded
            .try_into()
            .map_err(|_| "streaming upload payload hash must be SHA-256".to_string())?;
        Ok((pubkey.to_hex(), digest))
    }

    /// Verify a signed viewer token minted by the desktop app.
    ///
    /// The sandbox desktop is opened by a browser, which cannot sign a NIP-98
    /// `Authorization` header the way the app's own HTTP calls do. So the app
    /// signs a GET over the exact viewer URL and hands the browser that base64
    /// event as a `?t=` query token; this verifies it the same way `verify`
    /// checks a header, against the URL the broker itself published.
    ///
    /// `signed_url` is the viewer URL *without* the token — the URL the token
    /// was signed over. Returns the caller's hex pubkey on success.
    pub fn verify_token(&self, token_b64: &str, signed_url: &str) -> Result<String, String> {
        let json = {
            use base64::Engine;
            // base64url (unpadded): the token rides in a query string, and
            // standard base64's `+` `/` `=` are all mangled there (`+` decodes
            // to a space under form-urlencoding). URL-safe base64 is immune, so
            // the app mints and the broker reads that variant.
            let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(token_b64.trim())
                .map_err(|_| "viewer token is not valid base64url".to_string())?;
            String::from_utf8(bytes).map_err(|_| "viewer token is not valid UTF-8".to_string())?
        };

        // Freshness is enforced inside `verify_nip98_event` (±60s, the relay's
        // own window), so a viewer link stops working within a minute of being
        // minted. That is deliberately short: the link is a credential to a
        // logged-in desktop, so it must not be shareable or bookmarkable. The
        // app mints it at the instant the user opens the screen, so the browser
        // connects well inside the window; once the WebSocket is up it streams
        // regardless of the token's age.
        let pubkey = buzz_auth::verify_nip98_event(&json, signed_url, "GET", Some(&[]))
            .map_err(|e| format!("viewer token verification failed: {e}"))?;

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

    /// Does the agent's latest signed profile publish a NIP-OA relationship
    /// naming `owner_pubkey_hex` as its owner?
    ///
    /// A caller-generated NIP-OA signature is not sufficient: anyone can sign
    /// a statement claiming an unrelated agent. Requiring the agent-authored
    /// profile makes the relationship bilateral before it can authorize
    /// mounting that agent's persistent browser and files.
    pub async fn is_published_agent_owner(
        &self,
        owner_pubkey_hex: &str,
        agent_pubkey_hex: &str,
    ) -> bool {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(client) => client,
            Err(_) => return false,
        };
        let filter = serde_json::json!([{
            "kinds": [0],
            "authors": [agent_pubkey_hex],
            "limit": 1
        }]);
        let body = filter.to_string();
        let url = format!("{}/query", self.relay_url);
        let Some(auth) = self.sign_request("POST", &url, body.as_bytes()) else {
            return false;
        };
        let Ok(response) = client
            .post(&url)
            .header(axum::http::header::AUTHORIZATION, format!("Nostr {auth}"))
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
        else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        let Ok(events) = response.json::<Vec<nostr::Event>>().await else {
            return false;
        };
        events.first().is_some_and(|event| {
            profile_proves_agent_owner(event, owner_pubkey_hex, agent_pubkey_hex)
        })
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

fn profile_proves_agent_owner(
    event: &nostr::Event,
    owner_pubkey_hex: &str,
    agent_pubkey_hex: &str,
) -> bool {
    if event.kind != nostr::Kind::Metadata
        || event.pubkey.to_hex() != agent_pubkey_hex
        || event.verify().is_err()
    {
        return false;
    }
    let Ok(agent) = nostr::PublicKey::from_hex(agent_pubkey_hex) else {
        return false;
    };
    event.tags.iter().any(|tag| {
        let values = tag.as_slice();
        if values.first().map(String::as_str) != Some("auth") {
            return false;
        }
        let Ok(json) = serde_json::to_string(values) else {
            return false;
        };
        buzz_sdk::nip_oa::verify_auth_tag(&json, &agent)
            .map(|owner| owner.to_hex() == owner_pubkey_hex)
            .unwrap_or(false)
    })
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
        assert_eq!(v.public_urls, vec!["https://broker.example".to_string()]);
    }

    /// A caller signs over whichever advertised address it used — tunnel,
    /// container DNS name, or public proxy path — and any configured origin
    /// must verify.
    #[test]
    fn a_signature_over_any_configured_origin_verifies() {
        use base64::Engine;
        let v = Verifier::new(
            "https://relay.example".into(),
            "http://127.0.0.1:9310, http://buzz-sandbox-broker:9310".into(),
            nostr::Keys::generate(),
            true,
        );
        let keys = nostr::Keys::generate();
        for base in ["http://127.0.0.1:9310", "http://buzz-sandbox-broker:9310"] {
            let url = format!("{base}/sandboxes");
            let tags = vec![
                nostr::Tag::parse(["u", &url]).unwrap(),
                nostr::Tag::parse(["method", "GET"]).unwrap(),
            ];
            let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
                .tags(tags)
                .sign_with_keys(&keys)
                .unwrap();
            let header = base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_string(&event).unwrap());
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::header::AUTHORIZATION,
                HeaderValue::from_str(&format!("Nostr {header}")).unwrap(),
            );
            let pubkey = v
                .verify(&h, "GET", "/sandboxes", b"")
                .unwrap_or_else(|e| panic!("origin {base} must verify: {e}"));
            assert_eq!(pubkey, keys.public_key().to_hex());
        }
    }

    /// Pin the fs-API bug where the broker rebuilt the signed path from the
    /// axum `Query` extractor's *decoded* value instead of the raw query
    /// string as sent: the desktop app percent-encodes query values before
    /// signing (`path=%2Fworkspace`, not `path=/workspace`), and NIP-98 signs
    /// the exact URL string — decoding before reconstructing produces a
    /// different string than what was signed, and `verify` correctly rejects
    /// the mismatch. This is what `RawQuery` (not `Query`) in `main.rs`'s fs
    /// handlers must be built from.
    #[test]
    fn a_signature_over_a_percent_encoded_query_value_only_verifies_against_the_raw_query() {
        use base64::Engine;
        let v = verifier();
        let keys = nostr::Keys::generate();
        // What the desktop app actually signs: `/` inside the query value is
        // itself percent-encoded, matching its NON_ALPHANUMERIC-minus-`-_.~`
        // encoder.
        let signed_url = "https://broker.example/sandboxes/abc123/fs?path=%2Fworkspace";
        let tags = vec![
            nostr::Tag::parse(["u", signed_url]).unwrap(),
            nostr::Tag::parse(["method", "GET"]).unwrap(),
        ];
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap();
        let header = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(&event).unwrap());
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Nostr {header}")).unwrap(),
        );

        // Reconstructing from the RAW query (what `RawQuery` in main.rs's fs
        // handlers now uses) matches the signed URL and verifies.
        let raw_path = "/sandboxes/abc123/fs?path=%2Fworkspace";
        assert!(
            v.verify(&h, "GET", raw_path, b"").is_ok(),
            "the raw, still-encoded query must verify"
        );

        // Reconstructing from the DECODED query value (the bug: axum's
        // `Query` extractor unescapes `%2F` to `/` before this string is
        // rebuilt) does not match what was signed and must be refused.
        let decoded_path = "/sandboxes/abc123/fs?path=/workspace";
        assert!(
            v.verify(&h, "GET", decoded_path, b"").is_err(),
            "a path rebuilt from the decoded query value must not verify"
        );
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
    fn streaming_auth_returns_the_signature_bound_payload_hash() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let v = verifier();
        let keys = nostr::Keys::generate();
        let path = "/sandboxes/abc123/fs/file?path=%2Fworkspace%2Fmodel.glb";
        let url = format!("https://broker.example{path}");
        let expected = Sha256::digest(b"large-file-contents");
        let tags = vec![
            nostr::Tag::parse(["u", &url]).unwrap(),
            nostr::Tag::parse(["method", "PUT"]).unwrap(),
            nostr::Tag::parse(["payload", &hex::encode(expected)]).unwrap(),
        ];
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap();
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(&event).unwrap());
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Nostr {encoded}")).unwrap(),
        );

        let (pubkey, digest) = v
            .verify_streaming_body(&headers, "PUT", path)
            .expect("signed streaming request");
        assert_eq!(pubkey, keys.public_key().to_hex());
        assert_eq!(digest.as_slice(), expected.as_slice());
    }

    #[test]
    fn streaming_auth_requires_a_payload_commitment() {
        use base64::Engine;
        let v = verifier();
        let keys = nostr::Keys::generate();
        let path = "/sandboxes/abc123/fs/file?path=%2Fworkspace%2Fmodel.glb";
        let url = format!("https://broker.example{path}");
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags([
                nostr::Tag::parse(["u", &url]).unwrap(),
                nostr::Tag::parse(["method", "PUT"]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(&event).unwrap());
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Nostr {encoded}")).unwrap(),
        );

        let error = v.verify_streaming_body(&headers, "PUT", path).unwrap_err();
        assert!(error.contains("missing its payload hash"), "got: {error}");
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

    /// Mint a viewer token the way the desktop app does: a NIP-98 GET signature
    /// over the viewer URL, base64-encoded (the part after `Nostr `).
    fn mint_token(keys: &nostr::Keys, url: &str, created_at: i64) -> String {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let tags = vec![
            nostr::Tag::parse(["u", url]).unwrap(),
            nostr::Tag::parse(["method", "GET"]).unwrap(),
            nostr::Tag::parse(["payload", &hex::encode(Sha256::digest([]))]).unwrap(),
        ];
        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .custom_created_at(nostr::Timestamp::from(created_at as u64))
            .sign_with_keys(keys)
            .unwrap();
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&event).unwrap())
    }

    #[test]
    fn a_fresh_token_signed_over_the_url_verifies_to_its_signer() {
        let keys = nostr::Keys::generate();
        let url = "https://broker.example/sandboxes/abc123def456/desktop";
        let token = mint_token(&keys, url, chrono::Utc::now().timestamp());
        let pubkey = verifier().verify_token(&token, url).expect("valid token");
        assert_eq!(pubkey, keys.public_key().to_hex());
    }

    /// The terminal surface reuses the same token verification as the
    /// desktop, but signed over a `/terminal` URL rather than `/desktop` —
    /// confirms a token minted for one surface's URL verifies when checked
    /// against that same surface, matching how `main.rs`'s
    /// `verify_surface_token` builds `signed_url` as
    /// `{public_base}/sandboxes/{id}/{surface}`.
    #[test]
    fn a_terminal_token_verifies_against_the_terminal_url() {
        let keys = nostr::Keys::generate();
        let url = "https://broker.example/sandboxes/abc123def456/terminal";
        let token = mint_token(&keys, url, chrono::Utc::now().timestamp());
        let pubkey = verifier().verify_token(&token, url).expect("valid token");
        assert_eq!(pubkey, keys.public_key().to_hex());
    }

    /// A token signed for the desktop's URL must not verify against the
    /// terminal's — the two surfaces are gated by distinct signed URLs even
    /// though they share the same sandbox id, so a leaked desktop link cannot
    /// be replayed to open a shell.
    #[test]
    fn a_desktop_token_is_refused_against_the_terminal_url() {
        let keys = nostr::Keys::generate();
        let desktop_url = "https://broker.example/sandboxes/abc123def456/desktop";
        let terminal_url = "https://broker.example/sandboxes/abc123def456/terminal";
        let token = mint_token(&keys, desktop_url, chrono::Utc::now().timestamp());
        assert!(verifier().verify_token(&token, terminal_url).is_err());
    }

    #[test]
    fn a_token_for_a_different_sandbox_is_refused() {
        // A token is bound to the exact URL it was signed over, so it cannot be
        // replayed against another sandbox's screen.
        let keys = nostr::Keys::generate();
        let signed = "https://broker.example/sandboxes/aaaaaaaaaaaa/desktop";
        let other = "https://broker.example/sandboxes/bbbbbbbbbbbb/desktop";
        let token = mint_token(&keys, signed, chrono::Utc::now().timestamp());
        assert!(verifier().verify_token(&token, other).is_err());
    }

    #[test]
    fn an_expired_token_is_refused() {
        // A viewer link stops working within the NIP-98 freshness window, so a
        // captured link cannot be replayed later.
        let keys = nostr::Keys::generate();
        let url = "https://broker.example/sandboxes/abc123def456/desktop";
        let stale = chrono::Utc::now().timestamp() - 120;
        let token = mint_token(&keys, url, stale);
        assert!(verifier().verify_token(&token, url).is_err());
    }

    #[test]
    fn a_non_base64_token_is_refused() {
        assert!(verifier()
            .verify_token("!!!not base64!!!", "https://broker.example/x")
            .is_err());
    }

    fn agent_profile(agent: &nostr::Keys, owner: &nostr::Keys) -> nostr::Event {
        let auth_json = buzz_sdk::nip_oa::compute_auth_tag(owner, &agent.public_key(), "")
            .expect("owner and agent are distinct");
        let auth = buzz_sdk::nip_oa::parse_auth_tag(&auth_json).expect("valid auth tag");
        nostr::EventBuilder::new(nostr::Kind::Metadata, "{}")
            .tags([auth])
            .sign_with_keys(agent)
            .expect("profile signs")
    }

    #[test]
    fn agent_signed_profile_proves_its_published_owner() {
        let agent = nostr::Keys::generate();
        let owner = nostr::Keys::generate();
        let profile = agent_profile(&agent, &owner);
        assert!(profile_proves_agent_owner(
            &profile,
            &owner.public_key().to_hex(),
            &agent.public_key().to_hex()
        ));
    }

    #[test]
    fn unilateral_or_mismatched_owner_claims_do_not_prove_relationship() {
        let agent = nostr::Keys::generate();
        let owner = nostr::Keys::generate();
        let attacker = nostr::Keys::generate();
        let profile = agent_profile(&agent, &owner);

        assert!(!profile_proves_agent_owner(
            &profile,
            &attacker.public_key().to_hex(),
            &agent.public_key().to_hex()
        ));
        assert!(!profile_proves_agent_owner(
            &profile,
            &owner.public_key().to_hex(),
            &attacker.public_key().to_hex()
        ));

        // Even a cryptographically valid attacker claim is insufficient when
        // the agent did not publish it in its own signed profile.
        let attacker_claim =
            buzz_sdk::nip_oa::compute_auth_tag(&attacker, &agent.public_key(), "").unwrap();
        let attacker_tag = buzz_sdk::nip_oa::parse_auth_tag(&attacker_claim).unwrap();
        let self_signed_claim = nostr::EventBuilder::new(nostr::Kind::Metadata, "{}")
            .tags([attacker_tag])
            .sign_with_keys(&attacker)
            .unwrap();
        assert!(!profile_proves_agent_owner(
            &self_signed_claim,
            &attacker.public_key().to_hex(),
            &agent.public_key().to_hex()
        ));
    }
}
