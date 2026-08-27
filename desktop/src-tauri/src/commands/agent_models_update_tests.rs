use super::*;

#[test]
fn credential_merge_preserves_sibling_environment_values() {
    let current = BTreeMap::from([("SIBLING_TOKEN".to_string(), "keep-me".to_string())]);

    let merged = merged_credential_env(
        &current,
        "OPENAI_API_KEY".to_string(),
        "sk-secret-value".to_string(),
    )
    .unwrap();

    assert_eq!(
        merged.get("SIBLING_TOKEN").map(String::as_str),
        Some("keep-me")
    );
    assert_eq!(
        merged.get("OPENAI_API_KEY").map(String::as_str),
        Some("sk-secret-value")
    );
}

#[test]
fn credential_merge_rejects_empty_or_reserved_values_without_echoing_secret() {
    let empty_error = merged_credential_env(
        &BTreeMap::new(),
        "OPENAI_API_KEY".to_string(),
        "   ".to_string(),
    )
    .unwrap_err();
    assert_eq!(empty_error, "credential value cannot be empty");

    let secret = "should-never-appear-in-errors";
    let reserved_error = merged_credential_env(
        &BTreeMap::new(),
        "BUZZ_PRIVATE_KEY".to_string(),
        secret.to_string(),
    )
    .unwrap_err();
    assert!(!reserved_error.contains(secret));
}

fn provider_record(deployed: bool) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": "agent", "name": "Agent", "relay_url": "", "acp_command": "",
        "agent_command": "", "agent_args": [], "mcp_command": "",
        "turn_timeout_seconds": 0, "system_prompt": null, "created_at": "",
        "updated_at": "", "last_started_at": null, "last_stopped_at": null,
        "last_exit_code": null, "last_error": null
    }))
    .unwrap();
    record.backend = crate::managed_agents::BackendKind::Provider {
        id: "provider".into(),
        config: serde_json::json!({}),
    };
    record.backend_agent_id = deployed.then(|| "deployment".to_string());
    record
}

#[test]
fn deployed_provider_rejects_access_edits_that_cannot_be_revoked() {
    let error = ensure_access_policy_change_supported(&provider_record(true), true)
        .expect_err("deployed provider access edit must fail closed");
    assert!(error.contains("no explicit stop or revocation acknowledgement"));
}

#[test]
fn undeployed_provider_accepts_access_edits() {
    ensure_access_policy_change_supported(&provider_record(false), true)
        .expect("no running provider deployment can retain stale access");
}
