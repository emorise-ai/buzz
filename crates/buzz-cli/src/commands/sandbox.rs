//! `buzz sandbox` — the agent's own computer, managed like any other resource.
//!
//! A sandbox is a remote Linux desktop (browser included) the agent owns:
//! created when it needs one, extended while it works, destroyed when done.
//! The broker that runs them is reached via `BUZZ_SANDBOX_BROKER_URL`,
//! injected into the agent's environment by whatever launched it.

use crate::client::BuzzClient;
use crate::error::CliError;

pub async fn dispatch(cmd: crate::SandboxCmd, client: &BuzzClient) -> Result<(), CliError> {
    match cmd {
        crate::SandboxCmd::Create {
            broker,
            image,
            ttl,
            cpus,
            memory_mb,
        } => {
            // Self-service defaults: the sandbox belongs to the calling key,
            // and runs the desktop in tools-only mode — a long-lived tools
            // server plus a visible screen, with no second agent inside. The
            // owner env is what the in-sandbox tools server requires to serve.
            let owner = client.keys().public_key().to_hex();
            let mut body = serde_json::json!({
                "image": image,
                "owner": owner,
                "ttl_seconds": ttl,
                "env": {
                    "BUZZ_DEV_MCP_BIND": "0.0.0.0:9320",
                    "BUZZ_DEV_MCP_OWNER": owner,
                    "BUZZ_DESKTOP_ENABLED": "1",
                },
            });
            if let Some(cpus) = cpus {
                body["cpus"] = serde_json::json!(cpus);
            }
            if let Some(memory_mb) = memory_mb {
                body["memory_mb"] = serde_json::json!(memory_mb);
            }
            print_json(&client.sandbox_create(&broker.broker, &body).await?)
        }
        crate::SandboxCmd::List { broker } => {
            print_json(&client.sandbox_list(&broker.broker).await?)
        }
        crate::SandboxCmd::Status { broker, id } => {
            print_json(&client.sandbox_status(&broker.broker, &id).await?)
        }
        crate::SandboxCmd::Extend { broker, id, ttl } => {
            print_json(&client.sandbox_extend(&broker.broker, &id, ttl).await?)
        }
        crate::SandboxCmd::Destroy { broker, id } => {
            client.sandbox_destroy(&broker.broker, &id).await?;
            println!("{}", serde_json::json!({ "id": id, "destroyed": true }));
            Ok(())
        }
    }
}

fn print_json(value: &serde_json::Value) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| CliError::Other(e.to_string()))?
    );
    Ok(())
}
