//! Agent identity derived from the nsec, and the per-attempt generation token.
//!
//! Adapted from `buzz-backend-kubernetes/src/naming.rs`. The Kubernetes label
//! and annotation vocabulary is dropped — the sandbox broker owns container
//! labelling on its side — but identity derivation is unchanged, because the
//! spec ties object identity to the keypair rather than the display name.

use nostr::nips::nip19::FromBech32;

pub struct AgentIdentity {
    pubkey_hex: String,
}

impl AgentIdentity {
    /// Derive from the payload's `private_key_nsec`.
    ///
    /// Accepts bech32 `nsec1…`; a malformed or undecodable key is an immediate
    /// error, before any substrate read or mutation (§Deploy State Machine
    /// step 0). That ordering is what makes a bad key a refusal rather than a
    /// half-created sandbox.
    pub fn from_nsec(nsec: &str) -> Result<Self, String> {
        let secret = nostr::SecretKey::from_bech32(nsec.trim())
            .map_err(|_| "private_key_nsec is not a decodable nsec1 key".to_string())?;
        let keys = nostr::Keys::new(secret);
        Ok(Self {
            pubkey_hex: keys.public_key().to_hex(),
        })
    }

    /// Full 64-hex public key. Handed to the broker as the sandbox owner, so
    /// two agents sharing a display name stay distinct.
    pub fn pubkey_hex(&self) -> &str {
        &self.pubkey_hex
    }
}

/// A fresh generation token: 8 lowercase hex chars from the OS RNG.
///
/// Surfaces as `BUZZ_MANAGED_AGENT_START_NONCE`, so one create attempt and the
/// harness's lifecycle-frame correlator share one identity (§Launch data
/// tier 3). Never reused across attempts.
pub fn new_generation() -> String {
    use rand::RngExt;
    let n: u32 = rand::rng().random();
    format!("{n:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published NIP-19 example key — a spec test vector, not a secret.
    /// Deriving the pubkey rather than hardcoding both halves is the point: the
    /// test exercises the same derivation the deploy path depends on.
    const TEST_NSEC: &str = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";

    #[test]
    fn identity_derives_a_full_hex_pubkey() {
        let id = AgentIdentity::from_nsec(TEST_NSEC).expect("valid nsec");
        assert_eq!(id.pubkey_hex().len(), 64);
        assert!(id.pubkey_hex().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn identity_is_stable_for_the_same_key() {
        let a = AgentIdentity::from_nsec(TEST_NSEC).unwrap();
        let b = AgentIdentity::from_nsec(TEST_NSEC).unwrap();
        assert_eq!(a.pubkey_hex(), b.pubkey_hex());
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        let padded = format!("  {TEST_NSEC}\n");
        let a = AgentIdentity::from_nsec(&padded).unwrap();
        let b = AgentIdentity::from_nsec(TEST_NSEC).unwrap();
        assert_eq!(a.pubkey_hex(), b.pubkey_hex());
    }

    #[test]
    fn a_malformed_key_is_refused_not_silently_accepted() {
        for bad in ["", "not-a-key", "npub1xxx", "nsec1invalid"] {
            assert!(
                AgentIdentity::from_nsec(bad).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn generations_are_hex_and_do_not_repeat() {
        let set: std::collections::HashSet<String> = (0..200).map(|_| new_generation()).collect();
        assert!(set.len() > 190, "generation tokens collide too often");
        for g in &set {
            assert_eq!(g.len(), 8);
            assert!(g.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}
