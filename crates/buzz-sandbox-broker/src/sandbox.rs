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

#[derive(Debug, Deserialize)]
pub struct ExtendRequest {
    /// Seconds of lifetime wanted from *now*. The result is clamped so the
    /// sandbox's total lifetime never exceeds [`MAX_TTL_SECONDS`] from its
    /// creation — repeated extends must not make a sandbox immortal, because
    /// the TTL is the only bound on disk the host has (see `container_spec`).
    pub ttl_seconds: u64,
}

/// New expiry for an extend request: `now + ttl`, clamped to the lifetime
/// ceiling measured from creation.
///
/// The request's TTL goes through the same floor/ceiling as a create, then the
/// absolute cap applies. An extend can also *shorten* a sandbox's remaining
/// time — an agent that knows it needs only five more minutes may say so, and
/// the host gets its resources back sooner.
pub fn extended_expiry(now: i64, created_at: i64, ttl_seconds: u64) -> i64 {
    let requested = now + ttl_seconds.clamp(60, MAX_TTL_SECONDS) as i64;
    let cap = created_at + MAX_TTL_SECONDS as i64;
    requested.min(cap)
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

/// Directories the file API may touch inside a sandbox.
///
/// Everything else — `/etc`, `/proc`, `/root`, the container's own binaries —
/// is off limits even though the exec runs as the unprivileged agent user;
/// this is a second, independent boundary rather than relying on Unix
/// permissions alone.
const FS_ALLOWED_ROOTS: [&str; 2] = ["/workspace", "/home/agent"];

/// Validate a path handed to the file API before it ever reaches an exec
/// argv or a Docker archive call.
///
/// Rejects anything not absolute, any `.`/`..` segment (traversal, even
/// disguised inside a longer component boundary check would miss), and any
/// path outside the sandbox's own directories. A `\0` or newline is refused
/// outright — Docker's archive `path` query parameter and exec argv both
/// treat the string as a single token, so either character existing in a
/// "path" means it is not one.
pub fn validate_fs_path(path: &str) -> Result<(), String> {
    if path.is_empty() || !path.starts_with('/') {
        return Err("path must be absolute".to_string());
    }
    if path.contains('\0') || path.contains('\n') {
        return Err("path contains an invalid character".to_string());
    }
    for segment in path.split('/') {
        if segment == "." || segment == ".." {
            return Err("path must not contain \".\" or \"..\" segments".to_string());
        }
    }
    let under_allowed_root = FS_ALLOWED_ROOTS
        .iter()
        .any(|root| path == *root || path.starts_with(&format!("{root}/")));
    if !under_allowed_root {
        return Err(format!(
            "path must be under one of: {}",
            FS_ALLOWED_ROOTS.join(", ")
        ));
    }
    Ok(())
}

/// The desktop apps a sandbox's launch endpoint may open, each a fixed argv
/// vector — never assembled from caller input. A dock icon on the desktop
/// opens a real window for one of these under the window manager, rather than
/// switching a flat view; the launch endpoint exists only to name *which* of
/// exactly three known programs to start, never to run an arbitrary command.
pub const LAUNCH_APPS: &[(&str, &[&str])] = &[
    (
        "browser",
        &[
            "buzz-browser",
            "--new-window",
            "--user-data-dir=/home/agent/.config/chromium",
            "--password-store=basic",
            "--force-dark-mode",
            "--enable-features=WebUIDarkMode",
            "--no-sandbox",
            "--test-type",
        ],
    ),
    ("files", &["thunar", "/workspace"]),
    (
        "terminal",
        &["xfce4-terminal", "--working-directory=/workspace"],
    ),
];

/// Resolve a launch request's `app` name to its fixed argv, or reject it.
///
/// The returned slice is one of the constants in [`LAUNCH_APPS`] — no
/// caller-supplied string ever reaches the argv this builds a container exec
/// from.
pub fn launch_argv(app: &str) -> Result<&'static [&'static str], String> {
    LAUNCH_APPS
        .iter()
        .find(|(name, _)| *name == app)
        .map(|(_, argv)| *argv)
        .ok_or_else(|| {
            let allowed: Vec<&str> = LAUNCH_APPS.iter().map(|(name, _)| *name).collect();
            format!("unknown app {app:?}; allowed: {}", allowed.join(", "))
        })
}

/// The actions the window endpoint may perform on a desktop window, each a
/// fixed argv — the caller-supplied window id travels in the `BUZZ_WINDOW`
/// environment variable (validated by [`validate_window_id`] first), never
/// interpolated into the script text, mirroring how the fs API hands paths
/// to in-container scripts.
pub const WINDOW_ACTIONS: &[(&str, &[&str])] = &[
    (
        "activate",
        &["sh", "-c", "exec xdotool windowactivate \"$BUZZ_WINDOW\""],
    ),
    (
        "minimize",
        &["sh", "-c", "exec xdotool windowminimize \"$BUZZ_WINDOW\""],
    ),
    (
        "close",
        &["sh", "-c", "exec xdotool windowclose \"$BUZZ_WINDOW\""],
    ),
];

/// Resolve a window request's `action` name to its fixed argv, or reject it.
pub fn window_action_argv(action: &str) -> Result<&'static [&'static str], String> {
    WINDOW_ACTIONS
        .iter()
        .find(|(name, _)| *name == action)
        .map(|(_, argv)| *argv)
        .ok_or_else(|| {
            let allowed: Vec<&str> = WINDOW_ACTIONS.iter().map(|(name, _)| *name).collect();
            format!("unknown action {action:?}; allowed: {}", allowed.join(", "))
        })
}

/// Validate an X11 window id before it is handed to an in-container exec:
/// `0x`-prefixed hex or plain decimal, nothing else — a window id is the
/// only caller-controlled value the window endpoints forward, and this keeps
/// every accepted form inert in the `BUZZ_WINDOW` environment variable.
pub fn validate_window_id(window: &str) -> Result<(), String> {
    let digits = window.strip_prefix("0x").unwrap_or(window);
    let is_hex = window.starts_with("0x");
    if digits.is_empty()
        || digits.len() > 16
        || !digits.bytes().all(|b| {
            if is_hex {
                b.is_ascii_hexdigit()
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return Err("malformed window id".to_string());
    }
    Ok(())
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
    fn an_extend_adds_time_from_now() {
        let created = 1_000_000;
        let now = created + 600;
        assert_eq!(extended_expiry(now, created, 1800), now + 1800);
    }

    #[test]
    fn extends_never_exceed_the_lifetime_ceiling_from_creation() {
        let created = 1_000_000;
        let cap = created + MAX_TTL_SECONDS as i64;
        // Asking for the max near the end of life yields the cap, not now+max.
        let late = cap - 60;
        assert_eq!(extended_expiry(late, created, MAX_TTL_SECONDS), cap);
        // Repeated maximal extends converge on the same cap — immortality is
        // structurally impossible, not just discouraged.
        assert_eq!(extended_expiry(cap, created, u64::MAX), cap);
    }

    #[test]
    fn an_extend_may_shorten_the_remaining_time() {
        let created = 1_000_000;
        let now = created + 100;
        // The floor still applies: one minute is the least a sandbox can hold.
        assert_eq!(extended_expiry(now, created, 0), now + 60);
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

    #[test]
    fn fs_paths_under_the_allowed_roots_are_accepted() {
        assert!(validate_fs_path("/workspace").is_ok());
        assert!(validate_fs_path("/workspace/").is_ok());
        assert!(validate_fs_path("/workspace/project/src/main.rs").is_ok());
        assert!(validate_fs_path("/home/agent").is_ok());
        assert!(validate_fs_path("/home/agent/.bashrc").is_ok());
    }

    #[test]
    fn fs_paths_must_be_absolute() {
        assert!(validate_fs_path("").is_err());
        assert!(validate_fs_path("workspace/file").is_err());
        assert!(validate_fs_path("relative/path").is_err());
    }

    #[test]
    fn fs_paths_reject_dot_and_dotdot_segments() {
        assert!(validate_fs_path("/workspace/../etc/passwd").is_err());
        assert!(validate_fs_path("/workspace/./file").is_err());
        assert!(validate_fs_path("/workspace/a/../../etc").is_err());
        // A traversal disguised inside a longer, otherwise-legitimate-looking
        // component must still be caught by the exact-segment check.
        assert!(validate_fs_path("/workspace/..").is_err());
    }

    #[test]
    fn fs_paths_outside_the_allowed_roots_are_rejected() {
        assert!(validate_fs_path("/etc/passwd").is_err());
        assert!(validate_fs_path("/root/.ssh/id_rsa").is_err());
        assert!(validate_fs_path("/proc/1/environ").is_err());
        // A prefix collision must not pass: "/workspace-evil" is not under
        // "/workspace".
        assert!(validate_fs_path("/workspace-evil/file").is_err());
        assert!(validate_fs_path("/home/agent-evil/file").is_err());
    }

    #[test]
    fn fs_paths_reject_nul_and_newline() {
        assert!(validate_fs_path("/workspace/foo\0bar").is_err());
        assert!(validate_fs_path("/workspace/foo\nbar").is_err());
    }

    #[test]
    fn launch_argv_resolves_each_allowlisted_app_to_its_fixed_command() {
        assert_eq!(
            launch_argv("browser").unwrap(),
            &[
                "buzz-browser",
                "--new-window",
                "--user-data-dir=/home/agent/.config/chromium",
                "--password-store=basic",
                "--force-dark-mode",
                "--enable-features=WebUIDarkMode",
                "--no-sandbox",
                "--test-type",
            ]
        );
        assert_eq!(launch_argv("files").unwrap(), &["thunar", "/workspace"]);
        assert_eq!(
            launch_argv("terminal").unwrap(),
            &["xfce4-terminal", "--working-directory=/workspace"]
        );
    }

    #[test]
    fn launch_argv_rejects_anything_outside_the_allowlist() {
        // Includes attempts to smuggle a shell-meaningful string through the
        // app name itself, which must be refused the same as any other
        // unrecognized value — the name is looked up, never executed.
        for bad in ["", "vim", "browser ", "Browser", "; rm -rf /", "../etc"] {
            let err = launch_argv(bad).unwrap_err();
            assert!(
                err.contains("browser"),
                "error should list allowed apps: {err}"
            );
            assert!(err.contains("files"));
            assert!(err.contains("terminal"));
        }
    }

    #[test]
    fn window_action_argv_resolves_each_allowlisted_action_and_rejects_the_rest() {
        for (name, _) in WINDOW_ACTIONS {
            let argv = window_action_argv(name).unwrap();
            // The window id must reach the command only via the environment
            // variable — never as a literal argv slot a caller could fill.
            assert!(argv.iter().any(|a| a.contains("$BUZZ_WINDOW")));
        }
        for bad in ["", "activate ", "Activate", "kill", "; reboot"] {
            let err = window_action_argv(bad).unwrap_err();
            assert!(
                err.contains("activate") && err.contains("minimize") && err.contains("close"),
                "error should list allowed actions: {err}"
            );
        }
    }

    #[test]
    fn window_ids_accept_x11_forms_and_refuse_anything_shell_meaningful() {
        for good in ["0x04000007", "0xdeadBEEF", "67108871", "1"] {
            assert!(validate_window_id(good).is_ok(), "{good} should pass");
        }
        for bad in [
            "",
            "0x",
            "-1",
            "0x04000007; rm -rf /",
            "$(reboot)",
            "0x04 07",
            "abc",
            "0x11223344556677889900", // longer than any real X id
        ] {
            assert!(validate_window_id(bad).is_err(), "{bad:?} should fail");
        }
    }
}
