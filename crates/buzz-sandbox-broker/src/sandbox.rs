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
/// The caller who created the sandbox, as verified by NIP-98 at creation
/// time — distinct from `LABEL_OWNER` because the two are the same key only
/// when an agent self-creates its own box via the CLI. When the desktop app
/// creates a box on an agent's behalf, it signs the create call with the
/// *human* manager's key while the agent's own key is recorded as the owner
/// (see `create_sandbox`, main.rs). The manager is who the desktop's dock
/// authenticates as for the lifetime of that sandbox, so computer-use actions
/// need to admit this identity too, not just the owner.
pub const LABEL_MANAGER: &str = "com.buzz.sandbox.manager";
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
/// Grace window a keepalive bump buys, on every authenticated 2xx call
/// against a sandbox's own routes. Equal to the self-service TTL
/// (`DEFAULT_TTL_SECONDS`) — every real use resets the clock to a fresh
/// full window, still clamped by [`extended_expiry`] so continuous use
/// never makes a sandbox immortal past `created_at + MAX_TTL_SECONDS`.
pub const KEEPALIVE_GRACE_SECONDS: u64 = 60 * 30;
/// Minimum movement in expiry before the keepalive middleware republishes a
/// fresh kind:48200. Bumping the in-memory expiry map on every call is
/// cheap and is all the reaper needs; re-announcing to the relay on every
/// call would spam it, so only a move past this threshold since the last
/// publish triggers a fresh announcement.
pub const KEEPALIVE_REPUBLISH_THRESHOLD_SECONDS: i64 = 60;
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
    /// The NIP-98-verified caller who made the create call — see
    /// [`LABEL_MANAGER`]. `None` only in tests; `create_sandbox` always
    /// passes the caller.
    pub manager: Option<&'a str>,
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
        manager,
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
    if let Some(manager) = manager {
        labels.insert(LABEL_MANAGER.to_string(), serde_json::json!(manager));
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

/// Ceiling on `argv` for `POST /sandboxes/{id}/exec` — generous for a real
/// shell invocation, small enough that a request cannot be used to smuggle an
/// unbounded amount of data into the exec's `Cmd` array.
pub const EXEC_MAX_ARGV: usize = 64;
/// Ceiling on the summed byte length of all `argv` elements for an exec
/// request.
pub const EXEC_MAX_ARGV_BYTES: usize = 8 * 1024;
/// Default and clamp range for `timeout_secs` on an exec request. The broker
/// prepends `["timeout", "<secs>"]` to the caller's argv so a runaway command
/// is killed at the container level, and wraps the whole call in a
/// `tokio::time::timeout` five seconds past that as a belt-and-braces bound.
pub const EXEC_DEFAULT_TIMEOUT_SECS: u64 = 30;
pub const EXEC_MIN_TIMEOUT_SECS: u64 = 1;
pub const EXEC_MAX_TIMEOUT_SECS: u64 = 120;
/// Cap on stdout/stderr each, in an exec response. Large enough for normal
/// command output, small enough that a caller cannot use exec to pull
/// unbounded data out of a sandbox in one response.
pub const EXEC_MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub workdir: Option<String>,
}

/// Validated, clamped form of an [`ExecRequest`].
pub struct ResolvedExec {
    pub argv: Vec<String>,
    pub timeout_secs: u64,
    pub workdir: String,
}

/// Validate and clamp an exec request. Content of `argv` elements is not
/// inspected beyond size — the caller is the sandbox's own owner acting on
/// their own box — but the shape is: non-empty, bounded element count and
/// total size, and (if given) an absolute `workdir`.
pub fn resolve_exec(req: ExecRequest) -> Result<ResolvedExec, String> {
    if req.argv.is_empty() {
        return Err("argv must not be empty".to_string());
    }
    if req.argv.len() > EXEC_MAX_ARGV {
        return Err(format!("argv must have at most {EXEC_MAX_ARGV} elements"));
    }
    let total_bytes: usize = req.argv.iter().map(|a| a.len()).sum();
    if total_bytes > EXEC_MAX_ARGV_BYTES {
        return Err(format!(
            "argv totals {total_bytes} bytes, exceeding the {EXEC_MAX_ARGV_BYTES}-byte limit"
        ));
    }
    let timeout_secs = req
        .timeout_secs
        .unwrap_or(EXEC_DEFAULT_TIMEOUT_SECS)
        .clamp(EXEC_MIN_TIMEOUT_SECS, EXEC_MAX_TIMEOUT_SECS);
    let workdir = match req.workdir {
        Some(w) if !w.is_empty() => {
            if !w.starts_with('/') {
                return Err("workdir must be an absolute path".to_string());
            }
            w
        }
        _ => "/workspace".to_string(),
    };
    Ok(ResolvedExec {
        argv: req.argv,
        timeout_secs,
        workdir,
    })
}

/// One input action from `POST /sandboxes/{id}/input`, tagged by `type` in the
/// wire format. Coordinates and text are bounded here so nothing caller-shaped
/// reaches an xdotool argv unchecked, even though every field still travels as
/// its own argv element rather than through a shell.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputAction {
    Move {
        x: i64,
        y: i64,
    },
    Click {
        x: i64,
        y: i64,
        #[serde(default)]
        button: Option<String>,
    },
    DoubleClick {
        x: i64,
        y: i64,
    },
    Type {
        text: String,
    },
    Key {
        combo: String,
    },
    Scroll {
        x: i64,
        y: i64,
        direction: String,
        #[serde(default)]
        amount: Option<u32>,
    },
}

#[derive(Debug, Deserialize)]
pub struct InputRequest {
    pub actions: Vec<InputAction>,
}

pub const INPUT_MIN_ACTIONS: usize = 1;
pub const INPUT_MAX_ACTIONS: usize = 20;
pub const INPUT_MAX_TEXT_BYTES: usize = 4 * 1024;
pub const INPUT_COORD_MIN: i64 = 0;
pub const INPUT_COORD_MAX: i64 = 16384;
pub const INPUT_MIN_SCROLL_AMOUNT: u32 = 1;
pub const INPUT_MAX_SCROLL_AMOUNT: u32 = 10;

fn validate_coord(x: i64, y: i64) -> Result<(), String> {
    if !(INPUT_COORD_MIN..=INPUT_COORD_MAX).contains(&x)
        || !(INPUT_COORD_MIN..=INPUT_COORD_MAX).contains(&y)
    {
        return Err(format!(
            "coordinates must be within {INPUT_COORD_MIN}..={INPUT_COORD_MAX}"
        ));
    }
    Ok(())
}

/// `ctrl+t`-style key combos: letters, digits, underscore, and `+` as the
/// modifier separator, matching xdotool's own key-name grammar closely enough
/// that anything accepted here is inert as a shell argument regardless.
fn validate_key_combo(combo: &str) -> Result<(), String> {
    let len_ok = !combo.is_empty() && combo.len() <= 40;
    let chars_ok = combo
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'+');
    if !len_ok || !chars_ok {
        return Err(
            "key combo must be 1-40 characters of letters, digits, underscore, or +".to_string(),
        );
    }
    Ok(())
}

fn click_button_code(button: Option<&str>) -> Result<&'static str, String> {
    match button.unwrap_or("left") {
        "left" => Ok("1"),
        "middle" => Ok("2"),
        "right" => Ok("3"),
        other => Err(format!(
            "unknown button {other:?}; allowed: left, middle, right"
        )),
    }
}

fn scroll_direction_code(direction: &str) -> Result<&'static str, String> {
    match direction {
        "up" => Ok("4"),
        "down" => Ok("5"),
        other => Err(format!("unknown direction {other:?}; allowed: up, down")),
    }
}

/// Validate one input action and build its xdotool argv.
///
/// Every dynamic value (coordinates, text, combo) becomes its own argv
/// element — never concatenated into a shell string — mirroring the rest of
/// this module's exec calls.
pub fn input_action_argv(action: &InputAction) -> Result<Vec<String>, String> {
    match action {
        InputAction::Move { x, y } => {
            validate_coord(*x, *y)?;
            Ok(vec![
                "xdotool".to_string(),
                "mousemove".to_string(),
                x.to_string(),
                y.to_string(),
            ])
        }
        InputAction::Click { x, y, button } => {
            validate_coord(*x, *y)?;
            let code = click_button_code(button.as_deref())?;
            Ok(vec![
                "xdotool".to_string(),
                "mousemove".to_string(),
                x.to_string(),
                y.to_string(),
                "click".to_string(),
                code.to_string(),
            ])
        }
        InputAction::DoubleClick { x, y } => {
            validate_coord(*x, *y)?;
            Ok(vec![
                "xdotool".to_string(),
                "mousemove".to_string(),
                x.to_string(),
                y.to_string(),
                "click".to_string(),
                "--repeat".to_string(),
                "2".to_string(),
                "--delay".to_string(),
                "150".to_string(),
                "1".to_string(),
            ])
        }
        InputAction::Type { text } => {
            if text.len() > INPUT_MAX_TEXT_BYTES {
                return Err(format!(
                    "text exceeds the {INPUT_MAX_TEXT_BYTES}-byte limit"
                ));
            }
            Ok(vec![
                "xdotool".to_string(),
                "type".to_string(),
                "--delay".to_string(),
                "12".to_string(),
                "--".to_string(),
                text.clone(),
            ])
        }
        InputAction::Key { combo } => {
            validate_key_combo(combo)?;
            Ok(vec![
                "xdotool".to_string(),
                "key".to_string(),
                "--".to_string(),
                combo.clone(),
            ])
        }
        InputAction::Scroll {
            x,
            y,
            direction,
            amount,
        } => {
            validate_coord(*x, *y)?;
            let code = scroll_direction_code(direction)?;
            let amount = amount
                .unwrap_or(INPUT_MIN_SCROLL_AMOUNT)
                .clamp(INPUT_MIN_SCROLL_AMOUNT, INPUT_MAX_SCROLL_AMOUNT);
            Ok(vec![
                "xdotool".to_string(),
                "mousemove".to_string(),
                x.to_string(),
                y.to_string(),
                "click".to_string(),
                "--repeat".to_string(),
                amount.to_string(),
                code.to_string(),
            ])
        }
    }
}

/// Validate an input request's shape (action count) before any per-action
/// validation runs.
pub fn validate_input_request(req: &InputRequest) -> Result<(), String> {
    if req.actions.len() < INPUT_MIN_ACTIONS || req.actions.len() > INPUT_MAX_ACTIONS {
        return Err(format!(
            "actions must contain {INPUT_MIN_ACTIONS}..={INPUT_MAX_ACTIONS} entries"
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

/// Cap on the `url` field of a launch request.
pub const LAUNCH_URL_MAX_BYTES: usize = 2 * 1024;

/// Validate an optional launch URL against the app it targets.
///
/// Only `app == "browser"` may carry a `url` — any other app rejects it
/// outright, matching the contract's "400 otherwise". Valid only means: it
/// parses as an absolute URL and its scheme is `http` or `https`. The
/// returned string is the URL exactly as the caller sent it (not
/// re-serialized), appended as its own argv element — never interpolated
/// into a shell string.
pub fn validate_launch_url(app: &str, url: &str) -> Result<(), String> {
    if app != "browser" {
        return Err(format!(
            "url is only valid with app \"browser\", not {app:?}"
        ));
    }
    if url.len() > LAUNCH_URL_MAX_BYTES {
        return Err(format!("url exceeds the {LAUNCH_URL_MAX_BYTES}-byte limit"));
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("url is not a valid URL: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!(
            "url scheme must be http or https, got {:?}",
            parsed.scheme()
        ));
    }
    Ok(())
}

/// Append a validated launch URL to an app's fixed argv, guarded by a literal
/// `--` immediately before it.
///
/// `url::Url::parse` plus the http/https scheme check in
/// [`validate_launch_url`] already reject anything that looks like a
/// `--flag`, but the `--` is defense in depth: it means the appended element
/// can never be read as a switch by the launched program even if that
/// validation is ever loosened, since everything after `--` is positional by
/// convention (Chromium included).
pub fn append_launch_url<'a>(argv: &[&'a str], url: Option<&'a str>) -> Vec<&'a str> {
    let mut full: Vec<&str> = argv.to_vec();
    if let Some(url) = url {
        full.push("--");
        full.push(url);
    }
    full
}

/// Path inside the container that a screen recording is captured to and read
/// back from. Fixed, like `SCREENSHOT_PATH` in main.rs — there is exactly one
/// recording per sandbox, so there is nothing for a caller to name.
pub const RECORDING_PATH: &str = "/tmp/buzz-teach-recording.mp4";

/// Build the argv that starts screen recording inside a sandbox, backgrounded
/// so the exec that launches it returns immediately.
///
/// Runs under `sh -c` with a literal, caller-independent script — there is no
/// dynamic value anywhere in this command, so unlike `sandbox_exec`'s argv
/// wrapping there is nothing here for an injected value to escape from.
/// `setsid` detaches the ffmpeg process from the exec's own session so it
/// keeps running (and stays reachable by name for `recording_is_running_argv`
/// and `recording_stop_argv`) after this exec's stdout/stderr stream closes.
pub fn recording_start_argv() -> Vec<&'static str> {
    vec![
        "sh",
        "-c",
        "setsid ffmpeg -y -f x11grab -framerate 10 -i :1 -codec:v libx264 \
         -preset ultrafast -pix_fmt yuv420p \
         /tmp/buzz-teach-recording.mp4 > /tmp/buzz-teach-recording.log 2>&1 < /dev/null &",
    ]
}

/// Build the argv that reports whether a recording is currently in progress.
///
/// `pgrep -f` matches the ffmpeg command line by the fixed output path baked
/// into [`recording_start_argv`], so this and the start/stop commands agree
/// on identity without tracking a PID across requests. Exit code 0 means a
/// match was found; the caller (main.rs) reads only the exit code, not
/// stdout.
pub fn recording_is_running_argv() -> Vec<&'static str> {
    vec!["pgrep", "-f", "ffmpeg.*buzz-teach-recording.mp4"]
}

/// Build the argv that asks a running recording to stop gracefully.
///
/// `SIGINT` (not `SIGKILL`/`SIGTERM`... well, `SIGTERM` too — ffmpeg treats
/// both as "finish the file"): either lets ffmpeg flush its muxer and write a
/// valid `moov` atom before exiting, which a hard kill would skip, producing
/// an mp4 with no index. The caller polls `recording_is_running_argv` after
/// this until the process is gone rather than assuming the signal landed
/// instantly.
pub fn recording_stop_argv() -> Vec<&'static str> {
    vec!["pkill", "-INT", "-f", "ffmpeg.*buzz-teach-recording.mp4"]
}

/// Path inside the container that an uploaded narration track is written to
/// before muxing, and the merged result read back from. Both fixed, like
/// [`RECORDING_PATH`] — a stop call carries at most one audio upload and
/// produces at most one merged file, so there is nothing for a caller to name.
/// The audio path's extension varies with [`AUDIO_EXT_ALLOWLIST`] and is
/// substituted by [`audio_upload_path`]; the merged output is always `.mp4`
/// since muxing always re-packages into the same container format the screen
/// recording already uses.
pub const RECORDING_AUDIO_PATH_BASE: &str = "/tmp/buzz-teach-audio";
pub const RECORDING_MERGED_PATH: &str = "/tmp/buzz-teach-merged.mp4";

/// Extensions ffmpeg is expected to demux without extra flags, covering the
/// formats a browser's `MediaRecorder`/WebAudio capture or a plain WAV
/// encoder plausibly hands the broker. Checked against the caller-supplied
/// hint so nothing reaches a filename or exec argv that didn't come from this
/// fixed list.
pub const AUDIO_EXT_ALLOWLIST: &[&str] = &["wav", "webm", "ogg", "m4a", "mp3", "aac"];

/// Cap on an uploaded narration track. It is a short screen-recording
/// voiceover, not a general file upload — large enough for a long take at a
/// modest bitrate, small enough that a caller cannot use this path to push an
/// unbounded amount of data into a sandbox.
pub const RECORDING_AUDIO_MAX_BYTES: usize = 50 * 1024 * 1024;

/// Validate a caller-supplied audio extension hint against
/// [`AUDIO_EXT_ALLOWLIST`] and, if valid, return the fixed in-container path
/// the upload should be written to.
///
/// The hint travels as a query parameter (`?audio_ext=wav`), never as a path
/// or filename built from caller input — this function is the only place
/// that turns it into one, and it only ever appends one of the fixed
/// allowlisted suffixes to [`RECORDING_AUDIO_PATH_BASE`].
pub fn audio_upload_path(ext: &str) -> Result<String, String> {
    let normalized = ext.to_ascii_lowercase();
    if !AUDIO_EXT_ALLOWLIST.contains(&normalized.as_str()) {
        return Err(format!(
            "unsupported audio_ext {ext:?}; allowed: {}",
            AUDIO_EXT_ALLOWLIST.join(", ")
        ));
    }
    Ok(format!("{RECORDING_AUDIO_PATH_BASE}.{normalized}"))
}

/// Build the argv that muxes a just-recorded screen capture with an uploaded
/// narration track into one mp4.
///
/// `-c:v copy` re-packages the video stream without touching it — re-encoding
/// a screen capture a second time would cost CPU and quality for no reason,
/// since the capture is already a finished mp4. Only audio is encoded, to a
/// codec (`aac`) mp4 can actually contain, since the incoming track may be
/// wav/ogg/webm/etc. `-shortest` stops at whichever stream ends first: the
/// desktop's mic capture and the sandbox's screen capture start and stop as
/// two independent processes on two machines, so their durations are never
/// guaranteed to match exactly, and padding one to fit the other is not worth
/// the complexity for a v1 voiceover feature. `-y` overwrites any stale
/// merged file from a previous stop on the same sandbox.
pub fn recording_mux_argv<'a>(video_path: &'a str, audio_path: &'a str) -> Vec<&'a str> {
    vec![
        "ffmpeg",
        "-y",
        "-i",
        video_path,
        "-i",
        audio_path,
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        "-shortest",
        RECORDING_MERGED_PATH,
    ]
}

/// May `caller` act on a sandbox as its owner or manager?
///
/// Pure decision, factored out of `require_owner_or_manager` (main.rs) so it
/// can be exercised directly without a Docker inspect: `caller` matches the
/// owner label, or matches a *present* manager label. A manager label that is
/// simply absent (an older sandbox created before `LABEL_MANAGER` existed)
/// must never be treated as "anyone may act" — the `manager.is_some()` guard
/// is what keeps that case falling back to owner-only instead of failing
/// open.
pub fn is_owner_or_manager(caller: &str, owner: Option<&str>, manager: Option<&str>) -> bool {
    owner == Some(caller) || (manager.is_some() && manager == Some(caller))
}

/// One existing managed container's shape, as far as the create-dedupe
/// decision needs to know it — a pure projection of a Docker list entry so
/// the decision in [`existing_sandbox_for_owner`] can be exercised without a
/// Docker socket.
pub struct ExistingSandbox<'a> {
    pub owner: Option<&'a str>,
    pub running: bool,
}

/// Should `create_sandbox` reuse an existing container instead of making a
/// new one?
///
/// Per-owner dedup: an owner that already has a live (running) managed
/// sandbox gets that one back rather than a second box. Returns the index
/// of the first matching entry, so the caller can look up the full record
/// it needs to build a response from. `None` means proceed with a normal
/// create — either the owner is unset (nothing to dedupe against) or none
/// of their sandboxes are currently running.
///
/// Pure decision, factored out of `create_sandbox` (main.rs) the same way
/// `is_owner_or_manager` is factored out of `require_owner_or_manager`: the
/// broker's own test harness has no live Docker socket to list containers
/// through, so the *logic* is tested directly against a plain slice.
pub fn existing_sandbox_for_owner(
    owner: Option<&str>,
    existing: &[ExistingSandbox],
) -> Option<usize> {
    let owner = owner?;
    existing
        .iter()
        .position(|s| s.running && s.owner == Some(owner))
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
            manager: None,
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
            manager: Some("manager"),
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
        assert_eq!(spec["Labels"][LABEL_MANAGER], "manager");
    }

    #[test]
    fn disk_cap_is_omitted_unless_the_host_can_enforce_it() {
        let limits = Limits::resolve(&req(None, None, None));
        let base = || SpecInputs {
            image: "img",
            limits,
            env: &EMPTY_ENV,
            owner: None,
            manager: None,
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
            manager: None,
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
            manager: None,
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
            manager: None,
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
            manager: None,
            expires_at: 0,
            cpuset: "0-0",
            network: "buzz-sandboxes",
            disk_limit: None,
        });
        assert_eq!(spec["HostConfig"]["NetworkMode"], "buzz-sandboxes");
    }

    /// `manager: None` must omit the label entirely, not write an empty
    /// value — `require_owner_or_manager`'s fallback-to-owner-only behavior
    /// for older sandboxes depends on the label being *absent*, not blank.
    #[test]
    fn manager_label_is_omitted_when_absent() {
        let spec = container_spec(SpecInputs {
            image: "img",
            limits: Limits::resolve(&req(None, None, None)),
            env: &EMPTY_ENV,
            owner: Some("owner"),
            manager: None,
            expires_at: 0,
            cpuset: "0-0",
            network: "sandbox",
            disk_limit: None,
        });
        assert!(spec["Labels"].get(LABEL_MANAGER).is_none());
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

    fn exec_req(argv: &[&str], timeout_secs: Option<u64>, workdir: Option<&str>) -> ExecRequest {
        ExecRequest {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            timeout_secs,
            workdir: workdir.map(str::to_string),
        }
    }

    #[test]
    fn resolve_exec_rejects_empty_argv() {
        assert!(resolve_exec(exec_req(&[], None, None)).is_err());
    }

    #[test]
    fn resolve_exec_rejects_too_many_elements() {
        let many: Vec<String> = (0..EXEC_MAX_ARGV + 1).map(|i| i.to_string()).collect();
        let req = ExecRequest {
            argv: many,
            timeout_secs: None,
            workdir: None,
        };
        assert!(resolve_exec(req).is_err());
    }

    #[test]
    fn resolve_exec_rejects_oversized_argv_bytes() {
        let req = ExecRequest {
            argv: vec!["x".repeat(EXEC_MAX_ARGV_BYTES + 1)],
            timeout_secs: None,
            workdir: None,
        };
        assert!(resolve_exec(req).is_err());
    }

    #[test]
    fn resolve_exec_defaults_workdir_and_timeout() {
        let resolved = resolve_exec(exec_req(&["ls"], None, None)).unwrap();
        assert_eq!(resolved.workdir, "/workspace");
        assert_eq!(resolved.timeout_secs, EXEC_DEFAULT_TIMEOUT_SECS);
    }

    #[test]
    fn resolve_exec_clamps_timeout_to_the_allowed_range() {
        let low = resolve_exec(exec_req(&["ls"], Some(0), None)).unwrap();
        assert_eq!(low.timeout_secs, EXEC_MIN_TIMEOUT_SECS);
        let high = resolve_exec(exec_req(&["ls"], Some(9999), None)).unwrap();
        assert_eq!(high.timeout_secs, EXEC_MAX_TIMEOUT_SECS);
    }

    #[test]
    fn resolve_exec_requires_an_absolute_workdir() {
        assert!(resolve_exec(exec_req(&["ls"], None, Some("relative/path"))).is_err());
        assert!(resolve_exec(exec_req(&["ls"], None, Some("/workspace/sub"))).is_ok());
    }

    #[test]
    fn move_action_builds_xdotool_mousemove() {
        let argv = input_action_argv(&InputAction::Move { x: 10, y: 20 }).unwrap();
        assert_eq!(argv, vec!["xdotool", "mousemove", "10", "20"]);
    }

    #[test]
    fn click_action_maps_button_names_to_xdotool_codes() {
        for (button, code) in [
            (None, "1"),
            (Some("left"), "1"),
            (Some("middle"), "2"),
            (Some("right"), "3"),
        ] {
            let argv = input_action_argv(&InputAction::Click {
                x: 5,
                y: 6,
                button: button.map(str::to_string),
            })
            .unwrap();
            assert_eq!(argv, vec!["xdotool", "mousemove", "5", "6", "click", code]);
        }
    }

    #[test]
    fn click_action_rejects_unknown_button() {
        let err = input_action_argv(&InputAction::Click {
            x: 0,
            y: 0,
            button: Some("banana".to_string()),
        })
        .unwrap_err();
        assert!(err.contains("banana"));
    }

    #[test]
    fn double_click_action_builds_xdotool_repeat_click() {
        let argv = input_action_argv(&InputAction::DoubleClick { x: 1, y: 2 }).unwrap();
        assert_eq!(
            argv,
            vec![
                "xdotool",
                "mousemove",
                "1",
                "2",
                "click",
                "--repeat",
                "2",
                "--delay",
                "150",
                "1"
            ]
        );
    }

    #[test]
    fn type_action_guards_text_with_a_double_dash() {
        let argv = input_action_argv(&InputAction::Type {
            text: "hello --world".to_string(),
        })
        .unwrap();
        assert_eq!(
            argv,
            vec!["xdotool", "type", "--delay", "12", "--", "hello --world"]
        );
    }

    #[test]
    fn type_action_rejects_oversized_text() {
        let err = input_action_argv(&InputAction::Type {
            text: "x".repeat(INPUT_MAX_TEXT_BYTES + 1),
        })
        .unwrap_err();
        assert!(err.contains("4096") || err.contains("byte"));
    }

    #[test]
    fn key_action_validates_and_guards_the_combo() {
        let argv = input_action_argv(&InputAction::Key {
            combo: "ctrl+t".to_string(),
        })
        .unwrap();
        assert_eq!(argv, vec!["xdotool", "key", "--", "ctrl+t"]);
    }

    #[test]
    fn key_action_rejects_shell_meaningful_combos() {
        for bad in ["", "ctrl; rm -rf /", "$(reboot)", "a b", &"a".repeat(41)] {
            assert!(
                input_action_argv(&InputAction::Key {
                    combo: bad.to_string()
                })
                .is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn scroll_action_maps_direction_and_clamps_amount() {
        let up = input_action_argv(&InputAction::Scroll {
            x: 1,
            y: 2,
            direction: "up".to_string(),
            amount: Some(999),
        })
        .unwrap();
        assert_eq!(
            up,
            vec![
                "xdotool",
                "mousemove",
                "1",
                "2",
                "click",
                "--repeat",
                "10",
                "4"
            ]
        );
        let down = input_action_argv(&InputAction::Scroll {
            x: 1,
            y: 2,
            direction: "down".to_string(),
            amount: None,
        })
        .unwrap();
        assert_eq!(
            down,
            vec![
                "xdotool",
                "mousemove",
                "1",
                "2",
                "click",
                "--repeat",
                "1",
                "5"
            ]
        );
    }

    #[test]
    fn scroll_action_rejects_unknown_direction() {
        let err = input_action_argv(&InputAction::Scroll {
            x: 0,
            y: 0,
            direction: "sideways".to_string(),
            amount: None,
        })
        .unwrap_err();
        assert!(err.contains("sideways"));
    }

    #[test]
    fn coordinates_out_of_range_are_rejected() {
        assert!(input_action_argv(&InputAction::Move { x: -1, y: 0 }).is_err());
        assert!(input_action_argv(&InputAction::Move { x: 0, y: 16385 }).is_err());
        assert!(input_action_argv(&InputAction::Move { x: 16384, y: 16384 }).is_ok());
    }

    #[test]
    fn validate_input_request_enforces_action_count_bounds() {
        let none = InputRequest { actions: vec![] };
        assert!(validate_input_request(&none).is_err());

        let too_many = InputRequest {
            actions: (0..INPUT_MAX_ACTIONS + 1)
                .map(|_| InputAction::Move { x: 0, y: 0 })
                .collect(),
        };
        assert!(validate_input_request(&too_many).is_err());

        let ok = InputRequest {
            actions: vec![InputAction::Move { x: 0, y: 0 }],
        };
        assert!(validate_input_request(&ok).is_ok());
    }

    #[test]
    fn launch_url_is_only_valid_for_the_browser_app() {
        assert!(validate_launch_url("browser", "https://example.com").is_ok());
        assert!(validate_launch_url("browser", "http://example.com").is_ok());
        assert!(validate_launch_url("files", "https://example.com").is_err());
        assert!(validate_launch_url("terminal", "https://example.com").is_err());
    }

    #[test]
    fn launch_url_rejects_non_http_schemes_and_malformed_urls() {
        assert!(validate_launch_url("browser", "not a url").is_err());
        assert!(validate_launch_url("browser", "javascript:alert(1)").is_err());
        assert!(validate_launch_url("browser", "file:///etc/passwd").is_err());
        assert!(validate_launch_url("browser", "ftp://example.com").is_err());
    }

    #[test]
    fn launch_url_rejects_oversized_urls() {
        let huge = format!("https://example.com/{}", "a".repeat(LAUNCH_URL_MAX_BYTES));
        assert!(validate_launch_url("browser", &huge).is_err());
    }

    #[test]
    fn append_launch_url_guards_the_url_with_a_literal_double_dash() {
        let argv = launch_argv("browser").unwrap();
        let full = append_launch_url(argv, Some("https://example.com"));
        // The `--` must sit immediately before the URL, regardless of how
        // many fixed flags precede it, so the launched program can never
        // read the URL as a switch of its own.
        let dash_pos = full.iter().position(|a| *a == "--").expect("has a --");
        assert_eq!(full[dash_pos + 1], "https://example.com");
        assert_eq!(dash_pos + 2, full.len(), "url must be the final element");
    }

    #[test]
    fn append_launch_url_leaves_argv_untouched_without_a_url() {
        let argv = launch_argv("terminal").unwrap();
        let full = append_launch_url(argv, None);
        assert_eq!(full, argv);
        assert!(!full.contains(&"--"));
    }

    #[test]
    fn owner_may_act() {
        assert!(is_owner_or_manager("owner-pk", Some("owner-pk"), None));
        assert!(is_owner_or_manager(
            "owner-pk",
            Some("owner-pk"),
            Some("manager-pk")
        ));
    }

    #[test]
    fn manager_may_act_even_when_distinct_from_owner() {
        // The desktop's dock signs as the human manager, not the agent
        // owner — this is the case that motivated the whole gate.
        assert!(is_owner_or_manager(
            "manager-pk",
            Some("owner-pk"),
            Some("manager-pk")
        ));
    }

    #[test]
    fn an_unrelated_caller_is_refused() {
        assert!(!is_owner_or_manager(
            "someone-else-pk",
            Some("owner-pk"),
            Some("manager-pk")
        ));
        assert!(!is_owner_or_manager(
            "someone-else-pk",
            Some("owner-pk"),
            None
        ));
        assert!(!is_owner_or_manager("someone-else-pk", None, None));
    }

    /// A sandbox created before `LABEL_MANAGER` existed has no manager label
    /// at all — that must fall back to owner-only, never admit any caller
    /// just because the label happens to be missing.
    #[test]
    fn an_absent_manager_label_falls_back_to_owner_only() {
        assert!(is_owner_or_manager("owner-pk", Some("owner-pk"), None));
        assert!(!is_owner_or_manager("random-pk", Some("owner-pk"), None));
    }

    #[test]
    fn create_dedup_finds_a_running_sandbox_for_the_same_owner() {
        let existing = [
            ExistingSandbox {
                owner: Some("someone-else"),
                running: true,
            },
            ExistingSandbox {
                owner: Some("owner-pk"),
                running: true,
            },
        ];
        assert_eq!(
            existing_sandbox_for_owner(Some("owner-pk"), &existing),
            Some(1)
        );
    }

    #[test]
    fn create_dedup_ignores_stopped_sandboxes() {
        let existing = [ExistingSandbox {
            owner: Some("owner-pk"),
            running: false,
        }];
        assert_eq!(
            existing_sandbox_for_owner(Some("owner-pk"), &existing),
            None
        );
    }

    #[test]
    fn create_dedup_ignores_other_owners() {
        let existing = [ExistingSandbox {
            owner: Some("someone-else"),
            running: true,
        }];
        assert_eq!(
            existing_sandbox_for_owner(Some("owner-pk"), &existing),
            None
        );
    }

    /// A create with no owner has nothing to dedupe against — every such
    /// call is unconditionally a new sandbox, never matched to another
    /// ownerless one.
    #[test]
    fn create_dedup_is_a_no_op_without_an_owner() {
        let existing = [ExistingSandbox {
            owner: None,
            running: true,
        }];
        assert_eq!(existing_sandbox_for_owner(None, &existing), None);
    }

    #[test]
    fn create_dedup_picks_the_first_match() {
        let existing = [
            ExistingSandbox {
                owner: Some("owner-pk"),
                running: true,
            },
            ExistingSandbox {
                owner: Some("owner-pk"),
                running: true,
            },
        ];
        assert_eq!(
            existing_sandbox_for_owner(Some("owner-pk"), &existing),
            Some(0)
        );
    }

    #[test]
    fn recording_start_argv_targets_the_fixed_output_path_and_backgrounds() {
        let argv = recording_start_argv();
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[1], "-c");
        let script = argv[2];
        assert!(script.contains("ffmpeg"));
        assert!(script.contains(RECORDING_PATH));
        assert!(script.contains("-f x11grab"));
        assert!(script.contains("-i :1"));
        // Backgrounded and detached from the exec's own session so it
        // survives after the starting exec's stream closes.
        assert!(script.trim_end().ends_with('&'));
        assert!(script.contains("setsid"));
    }

    #[test]
    fn recording_is_running_argv_matches_by_the_fixed_output_path() {
        let argv = recording_is_running_argv();
        assert_eq!(argv[0], "pgrep");
        assert!(argv.iter().any(|a| a.contains("buzz-teach-recording.mp4")));
    }

    #[test]
    fn recording_stop_argv_sends_sigint_not_sigkill() {
        let argv = recording_stop_argv();
        assert_eq!(argv[0], "pkill");
        assert!(argv.contains(&"-INT"));
        assert!(!argv.contains(&"-KILL"));
        assert!(!argv.contains(&"-9"));
        assert!(argv.iter().any(|a| a.contains("buzz-teach-recording.mp4")));
    }

    /// The three argv builders must all identify the same process by the
    /// same fixed path — a mismatch here would mean start/is-running/stop
    /// silently disagree about what they're tracking.
    #[test]
    fn recording_argv_builders_agree_on_the_process_identity() {
        let start = recording_start_argv().join(" ");
        let is_running = recording_is_running_argv().join(" ");
        let stop = recording_stop_argv().join(" ");
        assert!(start.contains(RECORDING_PATH));
        assert!(is_running.contains("buzz-teach-recording.mp4"));
        assert!(stop.contains("buzz-teach-recording.mp4"));
    }

    #[test]
    fn audio_upload_path_accepts_every_allowlisted_extension() {
        for ext in AUDIO_EXT_ALLOWLIST {
            let path = audio_upload_path(ext).unwrap();
            assert_eq!(path, format!("{RECORDING_AUDIO_PATH_BASE}.{ext}"));
        }
    }

    #[test]
    fn audio_upload_path_is_case_insensitive() {
        assert_eq!(
            audio_upload_path("WAV").unwrap(),
            format!("{RECORDING_AUDIO_PATH_BASE}.wav")
        );
    }

    #[test]
    fn audio_upload_path_rejects_unlisted_extensions() {
        // Includes an attempt to smuggle a path traversal or shell-meaningful
        // string through the extension hint itself — it must be refused the
        // same as any other unrecognized value, never appended to the path.
        for bad in ["exe", "sh", "../../etc/passwd", "wav; rm -rf /", ""] {
            let err = audio_upload_path(bad).unwrap_err();
            assert!(
                err.contains("wav"),
                "error should list allowed extensions: {err}"
            );
        }
    }

    #[test]
    fn recording_mux_argv_copies_video_and_encodes_audio_only() {
        let argv = recording_mux_argv(RECORDING_PATH, "/tmp/buzz-teach-audio.wav");
        assert_eq!(argv[0], "ffmpeg");
        assert!(argv.contains(&"-i"));
        assert!(argv.contains(&RECORDING_PATH));
        assert!(argv.contains(&"/tmp/buzz-teach-audio.wav"));
        // Video is copied, never re-encoded.
        let cv_pos = argv.iter().position(|a| *a == "-c:v").expect("has -c:v");
        assert_eq!(argv[cv_pos + 1], "copy");
        // Audio is encoded to a codec mp4 can contain, regardless of source format.
        let ca_pos = argv.iter().position(|a| *a == "-c:a").expect("has -c:a");
        assert_eq!(argv[ca_pos + 1], "aac");
        assert!(argv.contains(&"-shortest"));
        assert!(argv.contains(&RECORDING_MERGED_PATH));
    }

    #[test]
    fn recording_mux_argv_places_video_input_before_audio_input() {
        // `-shortest` and stream mapping both assume input order; getting it
        // backwards wouldn't error, it would silently swap which track drives
        // the "-c:v copy" vs "-c:a aac" treatment... except ffmpeg doesn't
        // care about semantic order, it maps by stream type. This test pins
        // the argv shape so a future edit can't accidentally drop or
        // duplicate an -i flag.
        let argv = recording_mux_argv("/video.mp4", "/audio.wav");
        let positions: Vec<usize> = argv
            .iter()
            .enumerate()
            .filter(|(_, a)| **a == "-i")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 2, "exactly two -i flags");
        assert_eq!(argv[positions[0] + 1], "/video.mp4");
        assert_eq!(argv[positions[1] + 1], "/audio.wav");
    }
}
