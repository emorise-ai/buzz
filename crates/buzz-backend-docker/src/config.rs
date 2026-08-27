//! `provider_config` parsing and the JSON Schema the desktop renders.
//!
//! Invariant I2 governs this file: **no field may carry a credential**. The
//! desktop refuses any config whose schema declares a secret-shaped key, so a
//! field named `token` would break every deploy. Nothing here needs one: the
//! broker authenticates callers by their Buzz identity, so the provider signs
//! its own requests rather than presenting a shared secret.

/// Default sandbox lifetime. The harness also self-exits on inactivity; this
/// is the outer bound the broker enforces regardless.
const DEFAULT_INACTIVITY_SECONDS: u64 = 7200;

/// Validated `provider_config` for one deploy.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Broker base URL, e.g. `http://127.0.0.1:9310`.
    pub broker_url: String,
    /// Image to run. Digest-pinned in production; the broker enforces its own
    /// allowlist on top, so this is a request, not the last word.
    pub image: String,
    pub cpus: f64,
    pub memory_mb: u64,
    pub inactivity_seconds: u64,
    /// Broker base URL *as seen from inside a sandbox*, injected into the
    /// agent's environment as `BUZZ_SANDBOX_BROKER_URL` so `buzz sandbox …`
    /// can manage the agent's own computer. Distinct from `broker_url`, which
    /// is where *this provider* reaches the broker (often a local tunnel a
    /// container cannot see). Defaults to the broker container's DNS name on
    /// the sandbox network.
    pub agent_broker_url: String,
}

/// Parse and validate `provider_config`, rejecting anything the broker would
/// refuse later so a bad config fails before the nsec is sent.
pub fn parse(cfg: &serde_json::Value) -> Result<ProviderConfig, String> {
    let broker_url = required_string(cfg, "broker_url")?;
    if !broker_url.starts_with("http://") && !broker_url.starts_with("https://") {
        return Err("provider_config.broker_url must start with http:// or https://".into());
    }
    let image = required_string(cfg, "image")?;

    let cpus = optional_f64(cfg, "cpus")?.unwrap_or(2.0);
    if cpus <= 0.0 {
        return Err("provider_config.cpus must be greater than zero".into());
    }
    let memory_mb = optional_u64(cfg, "memory_mb")?.unwrap_or(4096);
    if memory_mb < 256 {
        return Err("provider_config.memory_mb must be at least 256".into());
    }
    let inactivity_seconds =
        optional_u64(cfg, "inactivity_seconds")?.unwrap_or(DEFAULT_INACTIVITY_SECONDS);
    if inactivity_seconds == 0 {
        // Mirrors the Kubernetes binding: an unbounded remote agent is an
        // opt-in this version does not offer, because nothing else would reap
        // a sandbox whose owner forgot it.
        return Err(
            "provider_config.inactivity_seconds must be greater than zero; an \
             indefinite sandbox has no reaper"
                .into(),
        );
    }

    let agent_broker_url = match cfg.get("agent_broker_url").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {
            let s = s.trim();
            if !s.starts_with("http://") && !s.starts_with("https://") {
                return Err(
                    "provider_config.agent_broker_url must start with http:// or https://".into(),
                );
            }
            s.trim_end_matches('/').to_string()
        }
        _ => DEFAULT_AGENT_BROKER_URL.to_string(),
    };

    Ok(ProviderConfig {
        broker_url: broker_url.trim_end_matches('/').to_string(),
        image,
        cpus,
        memory_mb,
        inactivity_seconds,
        agent_broker_url,
    })
}

/// Where a sandboxed agent finds the broker: the broker container's DNS name
/// on the shared sandbox network. Docker's embedded DNS resolves container
/// names on user-defined networks, so this works wherever the broker keeps its
/// conventional name; an operator whose broker lives elsewhere overrides it.
const DEFAULT_AGENT_BROKER_URL: &str = "http://buzz-sandbox-broker:9310";

fn required_string(cfg: &serde_json::Value, key: &str) -> Result<String, String> {
    match cfg.get(key).and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => Err(format!("provider_config.{key} is required")),
    }
}

/// Numeric fields arrive as `""` when a user clears the field in the desktop's
/// form, so an empty string means "unset", not "zero". Anything else
/// non-numeric is an in-band error rather than a silent default.
fn optional_u64(cfg: &serde_json::Value, key: &str) -> Result<Option<u64>, String> {
    match cfg.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => s
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|_| format!("provider_config.{key} is not a number: {s:?}")),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("provider_config.{key} must be a non-negative integer")),
        Some(other) => Err(format!(
            "provider_config.{key} must be a number, got {other}"
        )),
    }
}

fn optional_f64(cfg: &serde_json::Value, key: &str) -> Result<Option<f64>, String> {
    match cfg.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => s
            .trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| format!("provider_config.{key} is not a number: {s:?}")),
        Some(serde_json::Value::Number(n)) => Ok(n.as_f64()),
        Some(other) => Err(format!(
            "provider_config.{key} must be a number, got {other}"
        )),
    }
}

/// The schema the desktop renders as a config form.
///
/// Field names are lint-checked by the desktop for credential-shaped words
/// (I2) — `broker_token` would fail every deploy, which is why the token lives
/// in this process's environment instead.
pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "broker_url": {
                "type": "string",
                "title": "Sandbox broker URL",
                "description": "Base URL of the sandbox broker on the sandbox host, e.g. http://127.0.0.1:9310 over an authenticated tunnel. The broker holds the Docker credentials; this provider holds none.",
                "default": "http://127.0.0.1:9310"
            },
            "image": {
                "type": "string",
                "title": "Agent image",
                "description": "Image containing the buzz-acp runtime, e.g. ghcr.io/block/buzz-sprig-dev@sha256:<digest>. The broker enforces its own allowlist; an unlisted image is refused there.",
                "default": "ghcr.io/block/buzz-sprig-dev"
            },
            "cpus": {
                "type": "number",
                "title": "CPUs",
                "description": "CPU budget for the sandbox. The broker clamps this to its own ceiling.",
                "default": 2
            },
            "memory_mb": {
                "type": "integer",
                "title": "Memory (MB)",
                "description": "Memory budget for the sandbox. The broker clamps this to its own ceiling.",
                "default": 4096
            },
            "inactivity_seconds": {
                "type": "integer",
                "title": "Inactivity timeout (seconds)",
                "description": "The agent self-exits after this long with no dispatched work, and the broker reaps the sandbox at the same bound.",
                "default": DEFAULT_INACTIVITY_SECONDS
            },
            "agent_broker_url": {
                "type": "string",
                "title": "Broker URL inside a sandbox",
                "description": "How a sandboxed agent reaches the broker to manage its own computer (injected as BUZZ_SANDBOX_BROKER_URL). Defaults to the broker container's DNS name on the sandbox network.",
                "default": DEFAULT_AGENT_BROKER_URL
            }
        },
        "required": ["broker_url", "image"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> serde_json::Value {
        serde_json::json!({
            "broker_url": "http://127.0.0.1:9310",
            "image": "ghcr.io/block/buzz-sprig-dev@sha256:abc"
        })
    }

    #[test]
    fn broker_url_and_image_are_required() {
        let mut cfg = base();
        cfg.as_object_mut().unwrap().remove("broker_url");
        assert!(parse(&cfg).unwrap_err().contains("broker_url"));

        let mut cfg = base();
        cfg.as_object_mut().unwrap().remove("image");
        assert!(parse(&cfg).unwrap_err().contains("image"));
    }

    #[test]
    fn broker_url_must_be_http() {
        let mut cfg = base();
        cfg["broker_url"] = "127.0.0.1:9310".into();
        assert!(parse(&cfg).unwrap_err().contains("http://"));
    }

    #[test]
    fn trailing_slash_is_normalized_away() {
        let mut cfg = base();
        cfg["broker_url"] = "http://127.0.0.1:9310/".into();
        assert_eq!(parse(&cfg).unwrap().broker_url, "http://127.0.0.1:9310");
    }

    #[test]
    fn cleared_numeric_fields_fall_back_to_defaults() {
        // The desktop sends "" for a field the user cleared.
        let mut cfg = base();
        cfg["cpus"] = "".into();
        cfg["memory_mb"] = "".into();
        cfg["inactivity_seconds"] = "".into();
        let parsed = parse(&cfg).unwrap();
        assert_eq!(parsed.cpus, 2.0);
        assert_eq!(parsed.memory_mb, 4096);
        assert_eq!(parsed.inactivity_seconds, DEFAULT_INACTIVITY_SECONDS);
    }

    #[test]
    fn non_numeric_values_are_errors_not_silent_defaults() {
        let mut cfg = base();
        cfg["memory_mb"] = "lots".into();
        assert!(parse(&cfg).unwrap_err().contains("not a number"));
    }

    #[test]
    fn indefinite_lifetime_is_refused() {
        let mut cfg = base();
        cfg["inactivity_seconds"] = 0.into();
        assert!(parse(&cfg).unwrap_err().contains("no reaper"));
    }

    /// I2: the desktop refuses a schema declaring a credential-shaped field,
    /// so this must never regress — it would break every deploy.
    #[test]
    fn no_schema_field_looks_like_a_credential() {
        let schema = schema();
        let props = schema["properties"].as_object().unwrap();
        for key in props.keys() {
            for word in key.split(|c: char| !c.is_alphanumeric()) {
                let word = word.to_ascii_lowercase();
                assert!(
                    !matches!(
                        word.as_str(),
                        "secret" | "password" | "token" | "key" | "credential"
                    ),
                    "provider_config.{key} is credential-shaped; the desktop \
                     would refuse every deploy (I2)"
                );
            }
        }
    }

    #[test]
    fn schema_defaults_round_trip_through_parse() {
        let schema = schema();
        let props = &schema["properties"];
        let cfg = serde_json::json!({
            "broker_url": props["broker_url"]["default"],
            "image": props["image"]["default"],
            "cpus": props["cpus"]["default"],
            "memory_mb": props["memory_mb"]["default"],
            "inactivity_seconds": props["inactivity_seconds"]["default"],
        });
        // A prefilled form must not fail validation the moment it renders.
        parse(&cfg).expect("schema defaults must parse");
    }
}
