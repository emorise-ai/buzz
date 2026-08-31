//! `buzz sandbox` — the agent's own computer, managed like any other resource.
//!
//! A sandbox is a remote Linux desktop (browser included) the agent owns:
//! created when it needs one, extended while it works, destroyed when done.
//! The broker that runs them is reached via `BUZZ_SANDBOX_BROKER_URL`,
//! injected into the agent's environment by whatever launched it.

use crate::client::BuzzClient;
use crate::error::CliError;

const SELF_SERVICE_IMAGE: &str = "buzz-sprig-desktop";
const SELF_SERVICE_MEMORY_MB: u64 = 8192;

fn create_body(
    image: String,
    owner: &str,
    ttl: u64,
    cpus: Option<f64>,
    memory_mb: Option<u64>,
) -> serde_json::Value {
    let default_memory = (image == SELF_SERVICE_IMAGE).then_some(SELF_SERVICE_MEMORY_MB);
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
    if let Some(memory_mb) = memory_mb.or(default_memory) {
        body["memory_mb"] = serde_json::json!(memory_mb);
    }
    body
}

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
            let body = create_body(image, &owner, ttl, cpus, memory_mb);
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
        crate::SandboxCmd::Exec {
            broker,
            sandbox,
            timeout,
            workdir,
            command,
            argv,
        } => {
            let id = sandbox.require()?;
            let argv = match (command, argv.is_empty()) {
                (Some(command), true) => vec!["bash".to_string(), "-lc".to_string(), command],
                (None, false) => argv,
                (Some(_), false) => {
                    return Err(CliError::Usage(
                        "--command and trailing arguments are mutually exclusive".to_string(),
                    ));
                }
                (None, true) => {
                    return Err(CliError::Usage(
                        "exec requires either --command <string> or trailing arguments after --"
                            .to_string(),
                    ));
                }
            };
            let mut body = serde_json::json!({
                "argv": argv,
                "timeout_secs": timeout,
            });
            if let Some(workdir) = workdir {
                body["workdir"] = serde_json::json!(workdir);
            }
            print_json(&client.sandbox_exec(&broker.broker, &id, &body).await?)
        }
        crate::SandboxCmd::Screenshot {
            broker,
            sandbox,
            output,
        } => {
            let id = sandbox.require()?;
            let bytes = client.sandbox_screenshot(&broker.broker, &id).await?;
            let path = output.unwrap_or_else(|| default_screenshot_path(&id));
            std::fs::write(&path, &bytes).map_err(|e| {
                CliError::Other(format!("failed to write screenshot to {path}: {e}"))
            })?;
            print_json(&serde_json::json!({ "path": path, "bytes": bytes.len() }))
        }
        crate::SandboxCmd::Click {
            broker,
            sandbox,
            x,
            y,
            button,
        } => {
            let id = sandbox.require()?;
            let action = serde_json::json!([{ "type": "click", "x": x, "y": y, "button": button }]);
            print_json(&client.sandbox_input(&broker.broker, &id, action).await?)
        }
        crate::SandboxCmd::DoubleClick {
            broker,
            sandbox,
            x,
            y,
        } => {
            let id = sandbox.require()?;
            let action = serde_json::json!([{ "type": "double_click", "x": x, "y": y }]);
            print_json(&client.sandbox_input(&broker.broker, &id, action).await?)
        }
        crate::SandboxCmd::Move {
            broker,
            sandbox,
            x,
            y,
        } => {
            let id = sandbox.require()?;
            let action = serde_json::json!([{ "type": "move", "x": x, "y": y }]);
            print_json(&client.sandbox_input(&broker.broker, &id, action).await?)
        }
        crate::SandboxCmd::Type {
            broker,
            sandbox,
            text,
        } => {
            let id = sandbox.require()?;
            let action = serde_json::json!([{ "type": "type", "text": text }]);
            print_json(&client.sandbox_input(&broker.broker, &id, action).await?)
        }
        crate::SandboxCmd::Key {
            broker,
            sandbox,
            combo,
        } => {
            let id = sandbox.require()?;
            let action = serde_json::json!([{ "type": "key", "combo": combo }]);
            print_json(&client.sandbox_input(&broker.broker, &id, action).await?)
        }
        crate::SandboxCmd::Scroll {
            broker,
            sandbox,
            x,
            y,
            direction,
            amount,
        } => {
            let id = sandbox.require()?;
            let action = serde_json::json!([{
                "type": "scroll",
                "x": x,
                "y": y,
                "direction": direction,
                "amount": amount,
            }]);
            print_json(&client.sandbox_input(&broker.broker, &id, action).await?)
        }
        crate::SandboxCmd::Open {
            broker,
            sandbox,
            app,
            url,
        } => {
            let id = sandbox.require()?;
            let mut body = serde_json::json!({ "app": app });
            if let Some(url) = url {
                body["url"] = serde_json::json!(url);
            }
            print_json(&client.sandbox_launch(&broker.broker, &id, &body).await?)
        }
        crate::SandboxCmd::RecordStart { broker, sandbox } => {
            let id = sandbox.require()?;
            print_json(&client.sandbox_recording_start(&broker.broker, &id).await?)
        }
        crate::SandboxCmd::RecordStop {
            broker,
            sandbox,
            output,
        } => {
            let id = sandbox.require()?;
            let bytes = client.sandbox_recording_stop(&broker.broker, &id).await?;
            let path = output.unwrap_or_else(|| default_recording_path(&id));
            std::fs::write(&path, &bytes).map_err(|e| {
                CliError::Other(format!("failed to write recording to {path}: {e}"))
            })?;
            print_json(&serde_json::json!({ "path": path, "bytes": bytes.len() }))
        }
    }
}

/// Default screenshot filename: short enough to be readable, unique enough
/// not to collide across quick successive captures.
fn default_screenshot_path(id: &str) -> String {
    let id8: String = id.chars().take(8).collect();
    let unixts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("./sandbox-screenshot-{id8}-{unixts}.png")
}

/// Default recording filename, same shape as `default_screenshot_path`.
fn default_recording_path(id: &str) -> String {
    let id8: String = id.chars().take(8).collect();
    let unixts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("./sandbox-recording-{id8}-{unixts}.mp4")
}

fn print_json(value: &serde_json::Value) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| CliError::Other(e.to_string()))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{create_body, SELF_SERVICE_IMAGE, SELF_SERVICE_MEMORY_MB};

    #[test]
    fn self_service_request_defaults_to_eight_gib() {
        let body = create_body(SELF_SERVICE_IMAGE.to_string(), "owner", 1800, None, None);
        assert_eq!(body["memory_mb"], SELF_SERVICE_MEMORY_MB);
    }

    #[test]
    fn explicit_memory_overrides_self_service_default() {
        let body = create_body(
            SELF_SERVICE_IMAGE.to_string(),
            "owner",
            1800,
            None,
            Some(2048),
        );
        assert_eq!(body["memory_mb"], 2048);
    }

    #[test]
    fn other_images_retain_broker_memory_default() {
        let body = create_body(
            "other-allowlisted-image".to_string(),
            "owner",
            1800,
            None,
            None,
        );
        assert!(body.get("memory_mb").is_none());
    }
}
