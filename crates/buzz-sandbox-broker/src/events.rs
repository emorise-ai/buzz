//! Publishing sandbox lifecycle to the relay.
//!
//! Without this, a sandbox exists only inside the broker: Buzz can see that an
//! agent is online but not that it has a computer, nor how to open its screen.
//! Publishing the lifecycle as ordinary Nostr events puts that state where the
//! rest of Buzz already looks, so the desktop can render "this agent has a
//! computer — open its screen" from the relay rather than by querying a
//! service it otherwise knows nothing about.
//!
//! Publishing is **best-effort and non-blocking**. A sandbox that runs but goes
//! unannounced is a display problem; a sandbox that fails to start because the
//! relay was briefly unreachable is an outage. The former is always preferable,
//! so every failure here is logged and swallowed.

use buzz_core::kind::{KIND_SANDBOX_CREATED, KIND_SANDBOX_DESTROYED};
use nostr::{EventBuilder, Keys, Tag};
use std::time::Duration;

/// The broker signs its own announcements with its own keypair, generated at
/// startup unless one is configured. It is a participant in the community like
/// any other, not a privileged writer — the relay authenticates it the same way
/// it authenticates a person.
pub struct Publisher {
    relay_http: String,
    keys: Keys,
}

/// What a `sandbox created` announcement carries.
pub struct SandboxFacts<'a> {
    pub sandbox_id: &'a str,
    pub name: &'a str,
    pub image: &'a str,
    /// Hex pubkey of the agent this sandbox belongs to, so the desktop can join
    /// a sandbox to the agent card it already renders.
    pub owner: Option<&'a str>,
    pub cpus: f64,
    pub memory_mb: u64,
    pub expires_at: i64,
    /// Where a human can open the live desktop, when the image has one.
    pub viewer_url: Option<&'a str>,
}

impl Publisher {
    /// `relay_url` may be given as ws(s):// — the HTTP bridge lives on the same
    /// origin, so the scheme is normalized here rather than requiring the
    /// operator to configure the same host twice in two forms.
    pub fn new(relay_url: &str, keys: Keys) -> Result<Self, String> {
        let http = relay_url
            .trim_end_matches('/')
            .replacen("wss://", "https://", 1)
            .replacen("ws://", "http://", 1);
        Ok(Self {
            relay_http: http,
            keys,
        })
    }

    pub fn pubkey_hex(&self) -> String {
        self.keys.public_key().to_hex()
    }

    /// Announce that an agent now has a computer.
    pub async fn sandbox_created(&self, facts: SandboxFacts<'_>) {
        let mut tags = vec![
            Tag::parse(["d", facts.sandbox_id]).ok(),
            Tag::parse(["name", facts.name]).ok(),
            Tag::parse(["image", facts.image]).ok(),
            Tag::parse(["cpus", &facts.cpus.to_string()]).ok(),
            Tag::parse(["memory_mb", &facts.memory_mb.to_string()]).ok(),
            Tag::parse(["expires_at", &facts.expires_at.to_string()]).ok(),
        ];
        // `p` is the conventional pubkey reference, so a client filtering for
        // "events about this agent" finds the sandbox without a bespoke tag.
        if let Some(owner) = facts.owner {
            tags.push(Tag::parse(["p", owner]).ok());
        }
        if let Some(url) = facts.viewer_url {
            tags.push(Tag::parse(["viewer", url]).ok());
        }
        self.publish(KIND_SANDBOX_CREATED, tags.into_iter().flatten().collect())
            .await;
    }

    /// Announce that a sandbox is gone, and why.
    ///
    /// The reason matters to a reader: "expired" is routine, "destroyed" was
    /// somebody's decision, and a card that cannot tell them apart invites the
    /// wrong conclusion when an agent's computer disappears.
    pub async fn sandbox_destroyed(&self, sandbox_id: &str, owner: Option<&str>, reason: &str) {
        let mut tags = vec![
            Tag::parse(["d", sandbox_id]).ok(),
            Tag::parse(["reason", reason]).ok(),
        ];
        if let Some(owner) = owner {
            tags.push(Tag::parse(["p", owner]).ok());
        }
        self.publish(KIND_SANDBOX_DESTROYED, tags.into_iter().flatten().collect())
            .await;
    }

    /// The latest announced expiry for one sandbox, from the relay.
    ///
    /// Crash recovery for the broker's live expiry state: an extend rewrites
    /// broker memory and republishes the 48200, but the container label still
    /// holds the *initial* expiry. After a broker restart the relay's latest
    /// announcement is the only surviving record of an extension, so startup
    /// seeds from here rather than reaping an extended sandbox early.
    pub async fn fetch_latest_expiry(&self, sandbox_id: &str) -> Option<i64> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .ok()?;
        let body = serde_json::json!([{
            "kinds": [KIND_SANDBOX_CREATED],
            "#d": [sandbox_id],
            "limit": 1
        }])
        .to_string();
        let url = format!("{}/query", self.relay_http);
        let auth = self.sign_request("POST", &url, body.as_bytes())?;
        let resp = client
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Nostr {auth}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let events: serde_json::Value = resp.json().await.ok()?;
        let event = events.as_array()?.first()?;
        event
            .get("tags")?
            .as_array()?
            .iter()
            .filter_map(|t| t.as_array())
            .find(|t| t.first().and_then(|v| v.as_str()) == Some("expires_at"))
            .and_then(|t| t.get(1)?.as_str()?.parse::<i64>().ok())
    }

    /// Build a NIP-98 `Authorization` value for the broker's own HTTP request.
    ///
    /// Distinct from the event being published: this authenticates the *request*
    /// carrying it, which the relay requires on every door.
    fn sign_request(&self, method: &str, url: &str, body: &[u8]) -> Option<String> {
        use base64::Engine;
        use sha2::{Digest, Sha256};

        let mut tags = vec![
            Tag::parse(["u", url]).ok()?,
            Tag::parse(["method", method]).ok()?,
        ];
        if !body.is_empty() {
            let hash = hex::encode(Sha256::digest(body));
            tags.push(Tag::parse(["payload", &hash]).ok()?);
        }
        let auth = EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .ok()?;
        Some(base64::engine::general_purpose::STANDARD.encode(serde_json::to_string(&auth).ok()?))
    }

    /// Sign and POST one event. Never propagates an error to the caller.
    async fn publish(&self, kind: u32, tags: Vec<Tag>) {
        let event = match EventBuilder::new(nostr::Kind::from(kind as u16), "")
            .tags(tags)
            .sign_with_keys(&self.keys)
        {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, kind, "could not sign a sandbox event");
                return;
            }
        };

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "could not build an HTTP client");
                return;
            }
        };

        // The relay authenticates /events like every other HTTP door, so the
        // announcement is signed the same way a client would sign it. Posting
        // the event unsigned returns 401 — verified against the running relay.
        let url = format!("{}/events", self.relay_http);
        let body = match serde_json::to_string(&event) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, kind, "could not serialize a sandbox event");
                return;
            }
        };
        let Some(auth) = self.sign_request("POST", &url, body.as_bytes()) else {
            tracing::warn!(kind, "could not sign the publish request");
            return;
        };

        match client
            .post(&url)
            .header(reqwest::header::AUTHORIZATION, format!("Nostr {auth}"))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(kind, "published a sandbox event");
            }
            Ok(resp) => {
                // Logged, not raised: an unannounced sandbox still works.
                // The relay's own wording is carried through — "unknown kind"
                // and "invalid signature" need different fixes, and a bare
                // status code cannot tell an operator which they have.
                let status = resp.status().as_u16();
                let detail = resp.text().await.unwrap_or_default();
                tracing::warn!(
                    kind,
                    status,
                    detail = %detail.chars().take(300).collect::<String>(),
                    "the relay rejected a sandbox event"
                );
            }
            Err(e) => {
                tracing::warn!(kind, error = %e, "could not reach the relay to publish");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_urls_are_normalized_to_the_http_bridge() {
        // Operators configure one relay URL; both forms must land on the same
        // origin rather than requiring the host to be written twice.
        for (given, want) in [
            ("wss://relay.example/", "https://relay.example"),
            ("ws://localhost:3000", "http://localhost:3000"),
            ("https://relay.example", "https://relay.example"),
        ] {
            let p = Publisher::new(given, Keys::generate()).expect("valid");
            assert_eq!(p.relay_http, want, "for {given}");
        }
    }

    #[test]
    fn the_publisher_announces_under_the_key_it_is_given() {
        // The broker's announcements must come from one recognizable author,
        // so the key is supplied by the caller rather than minted here.
        let keys = Keys::generate();
        let expected = keys.public_key().to_hex();
        let p = Publisher::new("wss://r.example", keys).unwrap();
        assert_eq!(p.pubkey_hex(), expected);
        assert_eq!(p.pubkey_hex().len(), 64);
    }

    #[test]
    fn two_publishers_sharing_a_key_share_an_identity() {
        // Pinning the key is what makes the broker recognizable in the relay's
        // history rather than appearing as a new author on every deploy.
        let keys = Keys::generate();
        let a = Publisher::new("wss://r.example", keys.clone()).unwrap();
        let b = Publisher::new("wss://r.example", keys).unwrap();
        assert_eq!(a.pubkey_hex(), b.pubkey_hex());
    }
}
