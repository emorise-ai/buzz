//! Docker backend provider for Buzz remote agents (spec `docs/remote-agents.md`).
//!
//! One process per operation: read exactly one JSON request from stdin, write
//! exactly one JSON response to stdout, exit. The exit code carries exactly one
//! bit — 0 for a response that was produced, 1 for a failure to produce one.
//! Everything a caller needs to distinguish is *inside* the response's `ok`
//! field (§Provider Protocol).
//!
//! Unlike the Kubernetes binding, this provider does not talk to the substrate
//! directly. It calls a sandbox broker over HTTP; the broker holds the Docker
//! credentials and enforces every container limit. The provider therefore runs
//! on the launching machine carrying no substrate credentials at all: the
//! broker authenticates by Buzz identity, so the provider signs each request
//! with the agent's own key rather than presenting a shared secret.

mod broker;
mod config;
mod naming;

// The wire protocol and environment rules are the spec's, not Docker's, so they
// live in buzz-backend-common and are shared with every other binding rather
// than copied per substrate.
use buzz_backend_common::{env, wire};

use std::io::Read;
use wire::{Request, Response};

/// Why the shared environment rules fail on this substrate specifically.
const DIAGNOSTICS: env::SubstrateDiagnostics = env::SubstrateDiagnostics {
    non_posix_key_reason: "a non-POSIX name is not reliably visible to the \
                           process inside the container",
    env_too_large_reason: "the container would fail to exec",
};

/// The provider a shared-compute agent resolves to. Refused here as the spec's
/// backstop: a mesh agent runs on the relay's compute, so deploying it as a
/// sandbox would create a second, contending consumer of the same identity.
const RELAY_MESH_PROVIDER: &str = "relay-mesh";

fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        // No request means no response contract to honor. The one nonzero path.
        eprintln!("could not read the request from stdin: {e}");
        std::process::exit(1);
    }

    let response = respond(&input);
    println!(
        "{}",
        serde_json::to_string(&response).unwrap_or_else(|e| {
            format!(r#"{{"ok":false,"error":"could not serialize a response: {e}"}}"#)
        })
    );
}

fn respond(input: &str) -> Response {
    // Parsed as raw JSON first: the relay-mesh refusal must see the wire value,
    // and `AgentPayload` deliberately does not carry `provider`.
    let raw: serde_json::Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(e) => return Response::error(format!("request is not valid JSON: {e}")),
    };

    if let Some(refusal) = refuse_relay_mesh(&raw) {
        return Response::error(refusal);
    }

    let request: Request = match serde_json::from_value(raw) {
        Ok(request) => request,
        Err(e) => return Response::error(format!("could not understand the request: {e}")),
    };

    match request {
        Request::Info => Response::info(
            "docker",
            "Runs agents as sandboxed containers via a sandbox broker",
            config::schema(),
        ),
        Request::Deploy(deploy) => {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => return Response::error(format!("could not start the runtime: {e}")),
            };
            match runtime.block_on(deploy_agent(&deploy)) {
                Ok(agent_id) => Response::deployed(agent_id),
                Err(e) => Response::error(e),
            }
        }
    }
}

/// Refuse a shared-compute agent, reading the **raw wire value** and trimming
/// before comparing — a backstop that shares its bypass with the layer it backs
/// is not a backstop.
fn refuse_relay_mesh(raw: &serde_json::Value) -> Option<String> {
    let provider = raw.get("agent")?.get("provider")?.as_str()?;
    (provider.trim() == RELAY_MESH_PROVIDER).then(|| {
        "deploy refused: this agent is configured for shared compute \
         (relay-mesh), which runs on the relay rather than in a sandbox. \
         Switch the agent to a local runtime before deploying it to Docker."
            .to_string()
    })
}

/// Run one deploy to a terminal outcome.
async fn deploy_agent(request: &wire::DeployRequest) -> Result<String, String> {
    let cfg = config::parse(&request.provider_config)?;
    // Identity before any substrate contact: a malformed nsec is a refusal,
    // not a failed connection (§Deploy State Machine step 0).
    let identity = naming::AgentIdentity::from_nsec(&request.agent.private_key_nsec)?;

    // One generation per attempt. Doubles as the lifecycle correlator so the
    // sandbox's logs and the harness's frames share one identity.
    let generation = naming::new_generation();

    let mut environment = env::build_env(
        &request.agent,
        env::AuthoritativeInputs {
            generation: &generation,
            inactivity_seconds: Some(cfg.inactivity_seconds),
        },
        DIAGNOSTICS,
    )?;
    // Where `buzz sandbox …` finds the broker from inside the sandbox. The
    // operator's substrate knowledge wins over anything a persona guessed, so
    // this overwrites rather than defers.
    environment.insert(
        "BUZZ_SANDBOX_BROKER_URL".into(),
        cfg.agent_broker_url.clone(),
    );

    let sandbox_id = broker::create(
        &cfg.broker_url,
        broker::CreateSandbox {
            nsec: &request.agent.private_key_nsec,
            image: &cfg.image,
            // The pubkey, not the display name: object identity derives from
            // the key so two agents with the same label stay distinct.
            owner: Some(identity.pubkey_hex()),
            cpus: cfg.cpus,
            memory_mb: cfg.memory_mb,
            // The broker's outer bound matches the harness's own self-exit, so
            // a wedged harness is still reaped.
            ttl_seconds: cfg.inactivity_seconds,
            env: &environment,
        },
    )
    .await?;

    Ok(sandbox_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_is_pure_and_declares_the_protocol_version() {
        // `info` must not touch the substrate: the desktop calls it to render
        // the config form before a broker is known to exist.
        let resp = respond(r#"{"op":"info","request_id":"x"}"#);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["protocol_version"], wire::PROTOCOL_VERSION);
        assert!(json["config_schema"].is_object());
    }

    #[test]
    fn unknown_ops_are_in_band_errors() {
        let resp = respond(r#"{"op":"undeploy","request_id":"x"}"#);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["ok"], false);
        assert!(json["error"].as_str().unwrap().contains("understand"));
    }

    #[test]
    fn malformed_json_is_an_in_band_error() {
        let json = serde_json::to_value(respond("not json")).unwrap();
        assert_eq!(json["ok"], false);
        assert!(json["error"].as_str().unwrap().contains("valid JSON"));
    }

    #[test]
    fn relay_mesh_agents_are_refused_before_any_mutation() {
        for provider in ["relay-mesh", "  relay-mesh  "] {
            let req = serde_json::json!({
                "op": "deploy",
                "agent": { "provider": provider, "relay_url": "wss://r", "private_key_nsec": "x" },
                "provider_config": {}
            });
            let json = serde_json::to_value(respond(&req.to_string())).unwrap();
            assert_eq!(json["ok"], false, "provider {provider:?} must be refused");
            assert!(json["error"].as_str().unwrap().contains("shared compute"));
        }
    }

    #[test]
    fn a_deploy_missing_config_fails_before_reaching_the_broker() {
        // No broker_url => refused at parse, so no network call is attempted.
        let req = serde_json::json!({
            "op": "deploy",
            "agent": { "relay_url": "wss://r", "private_key_nsec": "x" },
            "provider_config": {}
        });
        let json = serde_json::to_value(respond(&req.to_string())).unwrap();
        assert_eq!(json["ok"], false);
        assert!(json["error"].as_str().unwrap().contains("broker_url"));
    }
}
