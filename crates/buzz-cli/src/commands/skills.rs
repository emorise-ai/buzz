//! `buzz skills save` — publish a taught skill (Teach a Task) as a signed
//! kind:48202 (`KIND_AGENT_SKILL`) event.
//!
//! `buzz-cli` has one generic write path per event shape, not a single
//! caller-supplied-kind escape hatch — this is that path for
//! `KIND_AGENT_SKILL`, the same way `notes`/`emoji`/`agents` each own their
//! kind. The relay scopes 48202 as `Scope::MessagesWrite` and treats it like
//! sandbox lifecycle status, not a channel or repository mutation (see
//! `buzz-relay/src/handlers/ingest.rs`), so this publishes a plain signed
//! event with no `h` tag.

use nostr::{EventBuilder, Kind, PublicKey, Tag};

use crate::client::{normalize_write_response, BuzzClient};
use crate::error::CliError;
use crate::validate::{read_file_or_stdin, validate_hex64};
use crate::SkillsCmd;

/// Build the kind:48202 event for a taught skill.
///
/// Pure and side-effect-free so its tag shape can be exercised without a
/// client or a relay: `d` (skill id), `name`, `p` (owner), `agent` (the
/// signer's own pubkey — the caller can never spoof this since it comes from
/// `self_pubkey`, not a flag), and an optional `recording` tag. `content` is
/// the skill body exactly as read from `--body-file`/stdin — never
/// re-serialized, so whitespace/formatting in the source JSON survives
/// byte-for-byte.
pub fn build_save_skill_event(
    id: &str,
    name: &str,
    owner_hex: &str,
    self_pubkey: &PublicKey,
    recording: Option<&str>,
    content: &str,
) -> Result<EventBuilder, CliError> {
    if id.is_empty() {
        return Err(CliError::Usage("skill id must not be empty".to_string()));
    }
    if name.is_empty() {
        return Err(CliError::Usage("skill name must not be empty".to_string()));
    }
    validate_hex64(owner_hex)?;

    let mut tags = vec![
        Tag::parse(["d", id]).map_err(|e| CliError::Other(format!("tag error: {e}")))?,
        Tag::parse(["name", name]).map_err(|e| CliError::Other(format!("tag error: {e}")))?,
        Tag::parse(["p", owner_hex]).map_err(|e| CliError::Other(format!("tag error: {e}")))?,
        Tag::parse(["agent", &self_pubkey.to_hex()])
            .map_err(|e| CliError::Other(format!("tag error: {e}")))?,
    ];
    if let Some(url) = recording {
        if url.is_empty() {
            return Err(CliError::Usage(
                "recording URL must not be empty when provided".to_string(),
            ));
        }
        tags.push(
            Tag::parse(["recording", url])
                .map_err(|e| CliError::Other(format!("tag error: {e}")))?,
        );
    }

    Ok(EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_AGENT_SKILL as u16),
        content,
    )
    .tags(tags))
}

pub async fn dispatch(cmd: SkillsCmd, client: &BuzzClient) -> Result<(), CliError> {
    match cmd {
        SkillsCmd::Save {
            id,
            name,
            owner,
            recording,
            body_file,
        } => {
            let content = read_file_or_stdin(&body_file)?;
            let self_pubkey = client.keys().public_key();
            let builder = build_save_skill_event(
                &id,
                &name,
                &owner,
                &self_pubkey,
                recording.as_deref(),
                &content,
            )?;
            let event = client.sign_event(builder)?;
            let raw = client.submit_event(event).await?;
            println!("{}", normalize_write_response(&raw));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    fn test_pubkey() -> PublicKey {
        Keys::generate().public_key()
    }

    #[test]
    fn save_event_carries_the_expected_tags_and_kind() {
        let pk = test_pubkey();
        let owner = "a".repeat(64);
        let builder =
            build_save_skill_event("deploy-app", "Deploy the app", &owner, &pk, None, "{}")
                .unwrap();
        let event = builder.sign_with_keys(&Keys::generate()).unwrap();

        assert_eq!(
            event.kind,
            Kind::Custom(buzz_core::kind::KIND_AGENT_SKILL as u16)
        );
        assert_eq!(event.content, "{}");

        let tag = |name: &str| -> Option<String> {
            event
                .tags
                .iter()
                .find(|t| t.as_slice().first().map(String::as_str) == Some(name))
                .and_then(|t| t.as_slice().get(1).cloned())
        };
        assert_eq!(tag("d").as_deref(), Some("deploy-app"));
        assert_eq!(tag("name").as_deref(), Some("Deploy the app"));
        assert_eq!(tag("p").as_deref(), Some(owner.as_str()));
        assert_eq!(tag("agent").as_deref(), Some(pk.to_hex().as_str()));
        assert!(tag("recording").is_none());
    }

    #[test]
    fn save_event_includes_the_recording_tag_when_given() {
        let pk = test_pubkey();
        let owner = "b".repeat(64);
        let builder = build_save_skill_event(
            "deploy-app",
            "Deploy the app",
            &owner,
            &pk,
            Some("https://relay.example.com/media/abc123.mp4"),
            "{}",
        )
        .unwrap();
        let event = builder.sign_with_keys(&Keys::generate()).unwrap();

        let recording = event
            .tags
            .iter()
            .find(|t| t.as_slice().first().map(String::as_str) == Some("recording"))
            .and_then(|t| t.as_slice().get(1).cloned());
        assert_eq!(
            recording.as_deref(),
            Some("https://relay.example.com/media/abc123.mp4")
        );
    }

    #[test]
    fn save_event_rejects_an_invalid_owner_pubkey() {
        let pk = test_pubkey();
        let err = build_save_skill_event("id", "name", "not-hex", &pk, None, "{}").unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
    }

    #[test]
    fn save_event_rejects_an_empty_id_or_name() {
        let pk = test_pubkey();
        let owner = "c".repeat(64);
        assert!(matches!(
            build_save_skill_event("", "name", &owner, &pk, None, "{}").unwrap_err(),
            CliError::Usage(_)
        ));
        assert!(matches!(
            build_save_skill_event("id", "", &owner, &pk, None, "{}").unwrap_err(),
            CliError::Usage(_)
        ));
    }

    #[test]
    fn save_event_rejects_an_empty_recording_url() {
        let pk = test_pubkey();
        let owner = "d".repeat(64);
        let err = build_save_skill_event("id", "name", &owner, &pk, Some(""), "{}").unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
    }
}
