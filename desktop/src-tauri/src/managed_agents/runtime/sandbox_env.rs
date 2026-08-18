//! Computer-use env vars for a spawned agent (`BUZZ_SANDBOX_BROKER_URL` /
//! `BUZZ_SANDBOX_ID`).
//!
//! Split out of `runtime.rs` to keep that file under the size ratchet; the
//! only call site is `spawn_agent_child`, which must apply [`apply_sandbox_env`]
//! AFTER its user-env layer — a persona/agent env override must never be able
//! to clobber the broker URL, which has to match the broker's own allowlisted
//! public URL or the agent's NIP-98-signed `buzz sandbox` calls fail auth.
//! `buzz-acp`'s "you have a computer" briefing is itself gated on
//! `BUZZ_SANDBOX_BROKER_URL` being set, so clearing both on detach is what
//! makes the agent correctly report having no computer again.

use tauri::{AppHandle, Manager};

use super::RespondToEnv;

/// Resolve the broker base and set/remove the computer-use env vars directly
/// on `command`, given the record's `sandbox_id` (`None` if unattached).
pub(crate) fn apply_sandbox_env(
    command: &mut std::process::Command,
    app: &AppHandle,
    sandbox_id: Option<&str>,
) {
    let state = app.state::<crate::app_state::AppState>();
    let broker_base = crate::sandbox_viewer::broker_base(&state);
    let (set, remove) = build_sandbox_env(sandbox_id, &broker_base);
    for (key, value) in &set {
        command.env(key, value);
    }
    for key in &remove {
        command.env_remove(key);
    }
}

/// Pure decision function for the computer-use env vars
/// (`BUZZ_SANDBOX_BROKER_URL` / `BUZZ_SANDBOX_ID`).
///
/// `Some(sandbox_id)` sets both — the harness's "you have a computer"
/// briefing and its `buzz sandbox` tools are gated on
/// `BUZZ_SANDBOX_BROKER_URL` being present. `None` removes both, belt-and-
/// suspenders against an inherited parent env var granting phantom
/// computer-use after detach.
///
/// `broker_base` is always computed (even when there's no sandbox to attach)
/// so the caller doesn't need a conditional — a `None` sandbox_id simply
/// discards it via the remove path.
fn build_sandbox_env(sandbox_id: Option<&str>, broker_base: &str) -> RespondToEnv {
    match sandbox_id {
        Some(id) => (
            vec![
                ("BUZZ_SANDBOX_BROKER_URL", broker_base.to_string()),
                ("BUZZ_SANDBOX_ID", id.to_string()),
            ],
            Vec::new(),
        ),
        None => (
            Vec::new(),
            vec!["BUZZ_SANDBOX_BROKER_URL", "BUZZ_SANDBOX_ID"],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::build_sandbox_env;

    #[test]
    fn sandbox_env_some_id_sets_broker_url_and_id() {
        let (set, remove) = build_sandbox_env(
            Some("sandbox-123"),
            "https://relay.staging.emorise.com/sandbox-viewer",
        );
        let set_map: std::collections::HashMap<_, _> = set.into_iter().collect();
        assert_eq!(
            set_map.get("BUZZ_SANDBOX_ID").map(String::as_str),
            Some("sandbox-123")
        );
        assert_eq!(
            set_map.get("BUZZ_SANDBOX_BROKER_URL").map(String::as_str),
            Some("https://relay.staging.emorise.com/sandbox-viewer")
        );
        assert!(remove.is_empty());
    }

    #[test]
    fn sandbox_env_none_id_removes_both() {
        let (set, remove) =
            build_sandbox_env(None, "https://relay.staging.emorise.com/sandbox-viewer");
        assert!(set.is_empty());
        assert!(remove.contains(&"BUZZ_SANDBOX_BROKER_URL"));
        assert!(remove.contains(&"BUZZ_SANDBOX_ID"));
    }
}
