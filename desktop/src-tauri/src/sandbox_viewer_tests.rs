//! Tests for `sandbox_viewer.rs`, split out to keep that file under the
//! 1000-line gate; `#[path]`-included from there, so `super::*` resolves
//! against `sandbox_viewer`'s own scope.

use super::{
    computer_window_hash_route, computer_window_label, self_service_create_body, to_base64url,
    validate_sandbox_id, ComputerWindowParams,
};
use base64::Engine;

#[test]
fn self_service_create_requests_eight_gib_without_changing_the_profile() {
    let pubkey = "deadbeef".repeat(8);
    let body = self_service_create_body(&pubkey);

    assert_eq!(body["image"], "buzz-sprig-desktop");
    assert_eq!(body["owner"], pubkey);
    assert_eq!(body["ttl_seconds"], 1800);
    assert_eq!(body["memory_mb"], 8192);
    assert_eq!(body["env"]["BUZZ_DEV_MCP_BIND"], "0.0.0.0:9320");
    assert_eq!(body["env"]["BUZZ_DEV_MCP_OWNER"], "deadbeef".repeat(8));
    assert_eq!(body["env"]["BUZZ_DESKTOP_ENABLED"], "1");
}

#[test]
fn reencodes_standard_base64_as_url_safe() {
    // A payload whose standard base64 contains + and / must come back with
    // only URL-safe characters (- _), and decode to the same bytes.
    let raw = b"\xfb\xff\xbf hello world >>";
    let std = base64::engine::general_purpose::STANDARD.encode(raw);
    let url = to_base64url(&std).unwrap();
    assert!(!url.contains('+') && !url.contains('/') && !url.contains('='));
    let back = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&url)
        .unwrap();
    assert_eq!(back, raw);
}

#[test]
fn computer_window_label_is_stable_and_unique_per_sandbox() {
    // `open_computer_window` dedups via `app.get_webview_window(&label)`,
    // so two calls for the same sandbox id must produce the exact same
    // label (or a second click would build a duplicate window instead of
    // focusing the existing one), while two different sandboxes must not
    // collide.
    let id = validate_sandbox_id("buzz-sandbox-fq5ijxh7lt").unwrap();
    assert_eq!(
        computer_window_label(id),
        computer_window_label(id),
        "label must be deterministic for the same sandbox id"
    );
    assert_eq!(
        computer_window_label(id),
        "computer-buzz-sandbox-fq5ijxh7lt"
    );
    assert_ne!(
        computer_window_label(id),
        computer_window_label("other-sandbox-id")
    );
}

#[test]
fn hash_route_with_no_params_has_no_query_string() {
    let route = computer_window_hash_route("sbx-1", &ComputerWindowParams::default());
    assert_eq!(route, "index.html#/computer/sbx-1");
}

#[test]
fn hash_route_round_trips_every_param() {
    let params = ComputerWindowParams {
        viewer_url: Some("https://relay.example.com/sandbox-viewer/sbx-1".to_string()),
        sandbox_name: Some("buzz-sandbox-fq5ijxh7lt".to_string()),
        owner_pubkey: Some("deadbeef".repeat(8)),
        agent_display_name: Some("Fern the Agent".to_string()),
        expires_at: Some(1_700_000_000),
    };
    let route = computer_window_hash_route("sbx-1", &params);

    assert!(route.starts_with("index.html#/computer/sbx-1?"));
    assert!(route.contains("viewerUrl=https%3A%2F%2Frelay.example.com%2Fsandbox-viewer%2Fsbx-1"));
    assert!(route.contains("sandboxName=buzz-sandbox-fq5ijxh7lt"));
    assert!(route.contains(&format!("ownerPubkey={}", "deadbeef".repeat(8))));
    assert!(route.contains("agentDisplayName=Fern%20the%20Agent"));
    assert!(route.contains("expiresAt=1700000000"));
}

#[test]
fn hash_route_omits_blank_and_absent_params() {
    // Empty strings from the frontend (e.g. an agent with no resolved
    // display name) must not produce an empty `foo=` param — omitted
    // entirely is what lets the frontend's "fall back to the store"
    // check (`param === null`) work correctly.
    let params = ComputerWindowParams {
        viewer_url: Some(String::new()),
        sandbox_name: None,
        owner_pubkey: Some("  ".to_string()).filter(|s| !s.trim().is_empty()),
        agent_display_name: None,
        expires_at: None,
    };
    let route = computer_window_hash_route("sbx-1", &params);
    assert_eq!(route, "index.html#/computer/sbx-1");
}
