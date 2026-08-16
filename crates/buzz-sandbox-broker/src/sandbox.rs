//! Sandbox policy: what a container is allowed to be.
//!
//! Every limit here is enforced broker-side. A caller states what it wants; it
//! never states *how* the container is built, and no field of a request
//! reaches Docker uninterpreted. That is the whole point of the broker sitting
//! between the provider and the daemon (SANDBOX-PLAN.md §Phase 2).

use serde::{Deserialize, Serialize};

/// Label every managed container carries. The broker's list/reap paths filter
/// on it, so a container without it is invisible to this service — which is
/// what keeps the host's other containers out of reach.
pub const MANAGED_LABEL: &str = "com.buzz.sandbox=1";
pub const LABEL_KEY: &str = "com.buzz.sandbox";
pub const LABEL_OWNER: &str = "com.buzz.sandbox.owner";
pub const LABEL_EXPIRES: &str = "com.buzz.sandbox.expires-at";

/// Ceilings the broker will not exceed regardless of what is asked for.
///
/// Sized against the measured host (8 cores, ~49 GB available alongside 83
/// production containers): one sandbox may take a quarter of the box, and the
/// concurrent cap bounds the total.
pub const MAX_CPUS: f64 = 4.0;
pub const MAX_MEMORY_MB: u64 = 8192;
pub const MAX_TTL_SECONDS: u64 = 60 * 60 * 8;
pub const MAX_CONCURRENT: usize = 6;
pub const DEFAULT_CPUS: f64 = 2.0;
pub const DEFAULT_MEMORY_MB: u64 = 4096;
pub const DEFAULT_TTL_SECONDS: u64 = 60 * 60;
/// Process cap. Build tooling is fork-heavy, so this is generous — it exists to
/// stop a fork bomb taking the host down, not to constrain normal work.
pub const PIDS_LIMIT: i64 = 2048;

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// Image to run. Checked against the allowlist — a caller cannot run an
    /// arbitrary image, because the container holds the agent's private key.
    pub image: String,
    /// Opaque owner tag used for quota accounting and listing.
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub cpus: Option<f64>,
    #[serde(default)]
    pub memory_mb: Option<u64>,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    /// Environment for the agent process. Passed through as given: the
    /// provider has already resolved the three-tier precedence and identity
    /// rules the spec requires, and re-deriving them here would fork that
    /// logic into a second implementation.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct SandboxSummary {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub owner: Option<String>,
    pub expires_at: Option<i64>,
}

/// Resolved, clamped limits for one sandbox.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub cpus: f64,
    pub memory_mb: u64,
    pub ttl_seconds: u64,
}

impl Limits {
    /// Clamp rather than reject. A caller asking for 64 CPUs gets the ceiling
    /// and a working sandbox; refusing would turn a policy difference into an
    /// outage for something the broker can satisfy in a bounded way.
    pub fn resolve(req: &CreateRequest) -> Self {
        Self {
            cpus: req.cpus.unwrap_or(DEFAULT_CPUS).clamp(0.5, MAX_CPUS),
            memory_mb: req
                .memory_mb
                .unwrap_or(DEFAULT_MEMORY_MB)
                .clamp(256, MAX_MEMORY_MB),
            ttl_seconds: req
                .ttl_seconds
                .unwrap_or(DEFAULT_TTL_SECONDS)
                .clamp(60, MAX_TTL_SECONDS),
        }
    }
}

/// Inputs to [`container_spec`].
pub struct SpecInputs<'a> {
    pub image: &'a str,
    pub limits: Limits,
    pub env: &'a std::collections::BTreeMap<String, String>,
    pub owner: Option<&'a str>,
    pub expires_at: i64,
    pub cpuset: &'a str,
    pub network: &'a str,
    /// Writable-layer cap (e.g. "30G"), or None where unsupported.
    pub disk_limit: Option<&'a str>,
}

/// Build the Docker create payload for one sandbox.
///
/// Everything security-relevant is decided here, not by the caller:
///
/// * **No entrypoint override.** The image's entrypoint execs the harness as
///   PID 1 so it receives SIGTERM directly. Overriding it is how a provider
///   accidentally puts a shell in front of the signal receiver — the same rule
///   the Kubernetes binding states at `pod.rs:111`.
/// * **`--cpuset-cpus` as well as `--cpus`.** Measured on the host: `--cpus`
///   throttles CPU time but leaves `nproc` reporting every core, so build tools
///   over-parallelize and thrash. Pinning a contiguous range makes the sandbox
///   *see* what it may use.
/// * **All capabilities dropped, no-new-privileges.** Verified on the host to
///   yield `CapEff: 0000000000000000`.
/// * **No host mounts and no docker socket.** A sandbox that could reach the
///   daemon would be able to create an unconstrained container.
pub fn container_spec(inputs: SpecInputs<'_>) -> serde_json::Value {
    let SpecInputs {
        image,
        limits,
        env,
        owner,
        expires_at,
        cpuset,
        network,
        disk_limit,
    } = inputs;
    let env_vec: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    let mut labels = serde_json::Map::new();
    labels.insert(LABEL_KEY.to_string(), serde_json::json!("1"));
    labels.insert(
        LABEL_EXPIRES.to_string(),
        serde_json::json!(expires_at.to_string()),
    );
    if let Some(owner) = owner {
        labels.insert(LABEL_OWNER.to_string(), serde_json::json!(owner));
    }

    let mut spec = serde_json::json!({
        "Image": image,
        "Env": env_vec,
        "Labels": labels,
        // Tini-style init so orphaned grandchildren are reaped inside the
        // sandbox rather than accumulating as zombies during long builds.
        "HostConfig": {
            "Init": true,
            "NanoCpus": (limits.cpus * 1e9) as i64,
            "CpusetCpus": cpuset,
            "Memory": (limits.memory_mb * 1024 * 1024) as i64,
            // Equal to Memory: no swap. Swapping would let a sandbox exceed its
            // memory budget by degrading the whole host's IO instead.
            "MemorySwap": (limits.memory_mb * 1024 * 1024) as i64,
            "PidsLimit": PIDS_LIMIT,
            "CapDrop": ["ALL"],
            "SecurityOpt": ["no-new-privileges"],
            "NetworkMode": network,
            "Binds": [],
            "AutoRemove": false,
            // The harness exits cleanly on SIGTERM; an intentional clean exit
            // must never be restarted (spec L1 item 5).
            "RestartPolicy": { "Name": "no" },
            "StopTimeout": 60
        }
    });

    // Writable-layer cap, when the host can enforce one.
    //
    // `StorageOpt.size` requires overlay2 on XFS with the `pquota` mount
    // option; on ext4 (this host) Docker rejects the create outright with
    // "--storage-opt is supported only for overlay over xfs". So it is opt-in
    // via BUZZ_SANDBOX_DISK rather than hardcoded.
    //
    // Consequence worth stating plainly: without it, a runaway build inside a
    // sandbox can fill the host disk that production sites share. Memory, CPU,
    // and PIDs are capped; disk is not. Mitigations are the TTL reaper and the
    // concurrency cap, neither of which bounds bytes written.
    if let Some(size) = disk_limit {
        spec["HostConfig"]["StorageOpt"] = serde_json::json!({ "size": size });
    }

    spec
}

/// Is this image allowed to hold an agent's private key?
///
/// Prefix match against the configured allowlist. Digest pinning is checked
/// separately so the operator can see *why* an image was refused.
pub fn image_allowed(image: &str, allowed_prefixes: &[String]) -> Result<(), String> {
    if allowed_prefixes.is_empty() {
        return Err("no sandbox images are allowlisted; set BUZZ_SANDBOX_IMAGES".into());
    }
    if !allowed_prefixes.iter().any(|p| image.starts_with(p)) {
        return Err(format!(
            "image {image:?} is not allowlisted; allowed prefixes: {}",
            allowed_prefixes.join(", ")
        ));
    }
    Ok(())
}

/// Assign a contiguous cpuset for a sandbox, round-robin over the host's cores.
///
/// Two sandboxes may share cores — that is fine and intended; `NanoCpus` still
/// bounds their share. The point of the pin is that each sandbox *sees* a
/// core count matching its budget.
pub fn assign_cpuset(host_cpus: usize, want: f64, slot: usize) -> String {
    let want = want.ceil().max(1.0) as usize;
    let width = want.min(host_cpus.max(1));
    let start = if host_cpus == 0 {
        0
    } else {
        (slot * width) % host_cpus
    };
    let end = start + width - 1;
    if end < host_cpus {
        format!("{start}-{end}")
    } else {
        // Wrap: fall back to the top `width` cores rather than emitting a
        // range Docker would reject.
        format!("{}-{}", host_cpus.saturating_sub(width), host_cpus - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static EMPTY_ENV: std::sync::LazyLock<std::collections::BTreeMap<String, String>> =
        std::sync::LazyLock::new(Default::default);

    fn req(cpus: Option<f64>, mem: Option<u64>, ttl: Option<u64>) -> CreateRequest {
        CreateRequest {
            image: "ghcr.io/block/buzz-sprig-dev@sha256:abc".into(),
            owner: None,
            cpus,
            memory_mb: mem,
            ttl_seconds: ttl,
            env: Default::default(),
        }
    }

    #[test]
    fn limits_clamp_to_the_ceiling_rather_than_refusing() {
        let l = Limits::resolve(&req(Some(64.0), Some(1_000_000), Some(u64::MAX)));
        assert_eq!(l.cpus, MAX_CPUS);
        assert_eq!(l.memory_mb, MAX_MEMORY_MB);
        assert_eq!(l.ttl_seconds, MAX_TTL_SECONDS);
    }

    #[test]
    fn limits_have_a_floor_so_a_sandbox_is_never_unusable() {
        let l = Limits::resolve(&req(Some(0.0), Some(0), Some(0)));
        assert_eq!(l.cpus, 0.5);
        assert_eq!(l.memory_mb, 256);
        assert_eq!(l.ttl_seconds, 60);
    }

    #[test]
    fn defaults_apply_when_unspecified() {
        let l = Limits::resolve(&req(None, None, None));
        assert_eq!(l.cpus, DEFAULT_CPUS);
        assert_eq!(l.memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(l.ttl_seconds, DEFAULT_TTL_SECONDS);
    }

    #[test]
    fn image_allowlist_rejects_unlisted_and_empty_config() {
        let allowed = vec!["ghcr.io/block/buzz-sprig".to_string()];
        assert!(image_allowed("ghcr.io/block/buzz-sprig-dev@sha256:x", &allowed).is_ok());
        assert!(image_allowed("docker.io/library/alpine", &allowed).is_err());
        assert!(image_allowed("ghcr.io/block/buzz-sprig", &[]).is_err());
    }

    #[test]
    fn spec_never_overrides_the_entrypoint() {
        let spec = container_spec(SpecInputs {
            image: "img",
            limits: Limits::resolve(&req(None, None, None)),
            env: &Default::default(),
            owner: None,
            expires_at: 0,
            cpuset: "0-1",
            network: "sandbox",
            disk_limit: None,
        });
        // Entrypoint/Cmd absent => the image's own entrypoint execs the
        // harness as PID 1 and receives SIGTERM directly.
        assert!(spec.get("Entrypoint").is_none());
        assert!(spec.get("Cmd").is_none());
    }

    #[test]
    fn spec_hardening_is_present_and_mounts_are_empty() {
        let spec = container_spec(SpecInputs {
            image: "img",
            limits: Limits::resolve(&req(Some(2.0), Some(1024), None)),
            env: &Default::default(),
            owner: Some("owner"),
            expires_at: 123,
            cpuset: "0-1",
            network: "sandbox",
            disk_limit: None,
        });
        let hc = &spec["HostConfig"];
        assert_eq!(hc["CapDrop"], serde_json::json!(["ALL"]));
        assert_eq!(hc["SecurityOpt"], serde_json::json!(["no-new-privileges"]));
        assert_eq!(hc["Binds"], serde_json::json!([]));
        assert_eq!(hc["PidsLimit"], serde_json::json!(PIDS_LIMIT));
        assert_eq!(hc["RestartPolicy"]["Name"], "no");
        // No swap: Memory == MemorySwap.
        assert_eq!(hc["Memory"], hc["MemorySwap"]);
        assert_eq!(hc["CpusetCpus"], "0-1");
        assert_eq!(spec["Labels"][LABEL_KEY], "1");
        assert_eq!(spec["Labels"][LABEL_OWNER], "owner");
    }

    #[test]
    fn disk_cap_is_omitted_unless_the_host_can_enforce_it() {
        let limits = Limits::resolve(&req(None, None, None));
        let base = || SpecInputs {
            image: "img",
            limits,
            env: &EMPTY_ENV,
            owner: None,
            expires_at: 0,
            cpuset: "0-1",
            network: "sandbox",
            disk_limit: None,
        };
        let without = container_spec(base());
        // ext4 hosts reject StorageOpt outright, so it must be absent, not empty.
        assert!(without["HostConfig"].get("StorageOpt").is_none());

        let with = container_spec(SpecInputs {
            disk_limit: Some("30G"),
            ..base()
        });
        assert_eq!(with["HostConfig"]["StorageOpt"]["size"], "30G");
    }

    /// The container must never be able to grow past its memory budget by
    /// swapping — that trades a bounded failure for host-wide IO degradation.
    #[test]
    fn swap_is_pinned_to_the_memory_limit() {
        let limits = Limits::resolve(&req(None, Some(2048), None));
        let spec = container_spec(SpecInputs {
            image: "img",
            limits,
            env: &EMPTY_ENV,
            owner: None,
            expires_at: 0,
            cpuset: "0-1",
            network: "sandbox",
            disk_limit: None,
        });
        let hc = &spec["HostConfig"];
        assert_eq!(hc["Memory"], 2048i64 * 1024 * 1024);
        assert_eq!(hc["Memory"], hc["MemorySwap"]);
    }

    /// Env is what carries the agent's identity, so it must reach the
    /// container verbatim and in `KEY=value` form.
    #[test]
    fn environment_is_passed_through_as_key_value_pairs() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("BUZZ_RELAY_URL".to_string(), "wss://relay".to_string());
        env.insert("A".to_string(), "1".to_string());
        let spec = container_spec(SpecInputs {
            image: "img",
            limits: Limits::resolve(&req(None, None, None)),
            env: &env,
            owner: None,
            expires_at: 0,
            cpuset: "0-0",
            network: "sandbox",
            disk_limit: None,
        });
        let got: Vec<&str> = spec["Env"]
            .as_array()
            .expect("env array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(got.contains(&"BUZZ_RELAY_URL=wss://relay"));
        assert!(got.contains(&"A=1"));
    }

    /// An expiry label the reaper cannot parse would leave a sandbox running
    /// forever, so it is stored as a plain integer string.
    #[test]
    fn the_expiry_label_round_trips_as_an_integer() {
        let spec = container_spec(SpecInputs {
            image: "img",
            limits: Limits::resolve(&req(None, None, None)),
            env: &EMPTY_ENV,
            owner: None,
            expires_at: 1_786_824_039,
            cpuset: "0-0",
            network: "sandbox",
            disk_limit: None,
        });
        let raw = spec["Labels"][LABEL_EXPIRES].as_str().expect("label");
        assert_eq!(raw.parse::<i64>().expect("parses"), 1_786_824_039);
    }

    /// The sandbox must land on its own network, never the host's default,
    /// or it could reach services it has no business reaching.
    #[test]
    fn the_configured_network_is_used() {
        let spec = container_spec(SpecInputs {
            image: "img",
            limits: Limits::resolve(&req(None, None, None)),
            env: &EMPTY_ENV,
            owner: None,
            expires_at: 0,
            cpuset: "0-0",
            network: "buzz-sandboxes",
            disk_limit: None,
        });
        assert_eq!(spec["HostConfig"]["NetworkMode"], "buzz-sandboxes");
    }

    #[test]
    fn cpuset_stays_within_the_host_core_range() {
        for slot in 0..12 {
            for want in [0.5, 1.0, 2.0, 4.0] {
                let set = assign_cpuset(8, want, slot);
                let (a, b) = set.split_once('-').expect("a range");
                let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
                assert!(a <= b, "{set} is inverted");
                assert!(b < 8, "{set} exceeds the host core count");
            }
        }
    }

    #[test]
    fn cpuset_width_tracks_the_requested_cpus() {
        // 2 CPUs => 2 visible cores, so nproc matches the budget.
        assert_eq!(assign_cpuset(8, 2.0, 0), "0-1");
        // Fractional rounds up to one whole core.
        assert_eq!(assign_cpuset(8, 0.5, 0), "0-0");
    }
}
