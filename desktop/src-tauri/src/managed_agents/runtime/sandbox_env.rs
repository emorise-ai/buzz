//! Computer-use env vars for a spawned agent (`BUZZ_SANDBOX_BROKER_URL` /
//! `BUZZ_SANDBOX_ID`), and the reconnect-on-restart liveness check that runs
//! just before them.
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

/// What the reconnect-on-restart liveness probe decided about a record's
/// stored `sandbox_id`, before it's handed to [`apply_sandbox_env`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SandboxLivenessOutcome {
    /// No `sandbox_id` was stored — nothing to check.
    Unattached,
    /// The broker confirmed the sandbox is still alive (2xx) — keep the id,
    /// the agent reconnects to its existing computer.
    Alive,
    /// The broker gave a definitive 404 — the box is gone. The caller must
    /// clear `sandbox_id` on the record before spawning.
    Gone,
    /// The broker was unreachable, or answered with something other than a
    /// clean 2xx/404 (network error, 5xx, auth failure). Best-effort: keep
    /// the id rather than risk clearing a still-live sandbox on a network
    /// blip. `reason` is logged by the caller.
    Unknown { reason: String },
}

/// Pure decision: given the probe result for a record's `sandbox_id`, what
/// should happen to it before spawn.
///
/// Split from the async probe itself (in `sandbox_viewer::sandbox_is_alive`)
/// so the alive/gone/unreachable decision is directly testable without an
/// HTTP mock, mirroring how [`build_sandbox_env`] separates decision from
/// the `AppHandle`/`Command` side effects.
pub(crate) fn decide_sandbox_liveness(
    sandbox_id: Option<&str>,
    probe: Option<Result<bool, String>>,
) -> SandboxLivenessOutcome {
    let Some(_id) = sandbox_id else {
        return SandboxLivenessOutcome::Unattached;
    };
    match probe {
        Some(Ok(true)) => SandboxLivenessOutcome::Alive,
        Some(Ok(false)) => SandboxLivenessOutcome::Gone,
        Some(Err(reason)) => SandboxLivenessOutcome::Unknown { reason },
        None => SandboxLivenessOutcome::Unknown {
            reason: "no probe result".to_string(),
        },
    }
}

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
    use super::{build_sandbox_env, decide_sandbox_liveness, SandboxLivenessOutcome};

    #[test]
    fn liveness_no_sandbox_id_is_unattached() {
        // Nothing to probe — the fast path most agents hit every start.
        assert_eq!(
            decide_sandbox_liveness(None, None),
            SandboxLivenessOutcome::Unattached
        );
        // A probe result with no id to check should never happen in practice,
        // but must still degrade to Unattached rather than panic.
        assert_eq!(
            decide_sandbox_liveness(None, Some(Ok(true))),
            SandboxLivenessOutcome::Unattached
        );
    }

    #[test]
    fn liveness_2xx_probe_keeps_the_id() {
        assert_eq!(
            decide_sandbox_liveness(Some("sandbox-123"), Some(Ok(true))),
            SandboxLivenessOutcome::Alive
        );
    }

    #[test]
    fn liveness_404_probe_clears_the_id() {
        assert_eq!(
            decide_sandbox_liveness(Some("sandbox-123"), Some(Ok(false))),
            SandboxLivenessOutcome::Gone
        );
    }

    #[test]
    fn liveness_unreachable_broker_keeps_the_id() {
        // A network error (or any non-404 error status) must never clear a
        // possibly-still-live sandbox — best-effort favors keeping the id.
        let outcome = decide_sandbox_liveness(
            Some("sandbox-123"),
            Some(Err(
                "could not reach the sandbox broker: connection refused".to_string(),
            )),
        );
        assert!(matches!(outcome, SandboxLivenessOutcome::Unknown { .. }));
    }

    #[test]
    fn liveness_missing_probe_result_keeps_the_id() {
        // Defensive case: an id was present but no probe ran (e.g. the probe
        // future never resolved). Must degrade to Unknown, not Gone.
        let outcome = decide_sandbox_liveness(Some("sandbox-123"), None);
        assert!(matches!(outcome, SandboxLivenessOutcome::Unknown { .. }));
    }

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
