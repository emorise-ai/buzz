//! Client for the sandbox broker's HTTP API.
//!
//! The broker holds the Docker credentials and enforces every container limit;
//! this client only states intent. That split is why the provider binary can
//! run on a laptop without carrying substrate credentials
//! (SANDBOX-PLAN.md §Phase 2).

use std::collections::BTreeMap;

/// Sign a request to the broker with the agent's own Buzz identity (NIP-98).
///
/// The broker authenticates callers the same way the relay does, so there is no
/// shared secret to distribute: the agent's nsec — which the provider already
/// holds in order to deploy it — is what proves who is asking.
fn sign_request(nsec: &str, method: &str, url: &str, body: &[u8]) -> Result<String, String> {
    use base64::Engine;
    use nostr::nips::nip19::FromBech32;
    use sha2::{Digest, Sha256};

    let secret = nostr::SecretKey::from_bech32(nsec.trim())
        .map_err(|_| "cannot sign the broker request: the agent key is not a valid nsec")?;
    let keys = nostr::Keys::new(secret);

    let mut tags = vec![
        nostr::Tag::parse(["u", url])
            .map_err(|e| format!("could not build the auth event: {e}"))?,
        nostr::Tag::parse(["method", method])
            .map_err(|e| format!("could not build the auth event: {e}"))?,
    ];
    if !body.is_empty() {
        let hash = hex::encode(Sha256::digest(body));
        tags.push(
            nostr::Tag::parse(["payload", &hash])
                .map_err(|e| format!("could not build the auth event: {e}"))?,
        );
    }

    let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
        .tags(tags)
        .sign_with_keys(&keys)
        .map_err(|e| format!("could not sign the broker request: {e}"))?;
    let json = serde_json::to_string(&event)
        .map_err(|e| format!("could not serialize the auth event: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(json))
}

/// One sandbox creation request, as the broker expects it.
pub struct CreateSandbox<'a> {
    /// The agent's key, used to sign the request. The broker authenticates by
    /// Buzz identity, so the agent asks for its own sandbox.
    pub nsec: &'a str,
    pub image: &'a str,
    pub owner: Option<&'a str>,
    pub cpus: f64,
    pub memory_mb: u64,
    pub ttl_seconds: u64,
    pub env: &'a BTreeMap<String, String>,
}

/// Ask the broker for a sandbox. Returns the broker's id for it.
pub async fn create(base_url: &str, req: CreateSandbox<'_>) -> Result<String, String> {
    let body = serde_json::json!({
        "image": req.image,
        "owner": req.owner,
        "cpus": req.cpus,
        "memory_mb": req.memory_mb,
        "ttl_seconds": req.ttl_seconds,
        "env": req.env,
    });

    let client = reqwest::Client::builder()
        // A create that has not answered in two minutes has not started a
        // container the caller can rely on; the deploy op's own budget is
        // larger, so this fails fast enough to report usefully.
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("could not build an HTTP client: {e}"))?;

    let url = format!("{base_url}/sandboxes");
    let payload =
        serde_json::to_vec(&body).map_err(|e| format!("could not serialize the request: {e}"))?;
    let auth = sign_request(req.nsec, "POST", &url, &payload)?;

    let resp = client
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Nostr {auth}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(payload)
        .send()
        .await
        .map_err(|e| format!("could not reach the sandbox broker at {base_url}: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        // Surface the broker's own wording — "image is not allowlisted",
        // "sandbox limit reached" — rather than a generic failure.
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| text.trim().to_string());
        return Err(format!(
            "sandbox broker refused the deploy ({status}): {detail}"
        ));
    }

    serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("could not parse the broker response: {e}"))?
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "the broker did not return a sandbox id".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_request_carries_the_method_url_and_payload_hash() {
        // The broker verifies all three, so a signature over the wrong body or
        // URL is rejected — that is what stops a captured header being reused.
        use nostr::ToBech32;
        let nsec = nostr::Keys::generate()
            .secret_key()
            .to_bech32()
            .expect("bech32");
        let auth = sign_request(&nsec, "POST", "https://broker/sandboxes", b"{}").expect("signs");
        use base64::Engine;
        let json = base64::engine::general_purpose::STANDARD
            .decode(&auth)
            .expect("base64");
        let text = String::from_utf8(json).expect("utf8");
        assert!(text.contains("https://broker/sandboxes"));
        assert!(text.contains("POST"));
        assert!(text.contains("payload"));
    }

    #[test]
    fn a_malformed_agent_key_cannot_be_signed_with() {
        assert!(sign_request("not-an-nsec", "POST", "https://broker/x", b"{}").is_err());
    }
}
