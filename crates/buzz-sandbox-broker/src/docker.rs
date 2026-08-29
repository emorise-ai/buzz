//! A minimal Docker Engine API client over the unix socket.
//!
//! Four operations — create, start, inspect, remove — driven directly over
//! Docker's HTTP-over-unix-socket API rather than through a Docker SDK. The
//! surface this broker needs is small, and this service is the one component
//! that can create containers, so keeping its dependency (and audit) footprint
//! small is deliberate.
//!
//! The socket is never exposed to sandboxes. It is mounted into the broker
//! only, and no request path lets a caller pass raw Docker arguments through
//! (see `container_spec` in `sandbox.rs`).

use http_body_util::{BodyExt, Full};
use hyper::body::{Buf, Bytes};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::path::PathBuf;

/// Docker Engine API version to pin. Pinning avoids a daemon upgrade silently
/// changing response shapes under us; 1.43 is available on Docker 24+ and the
/// host runs 29.x.
const API_VERSION: &str = "v1.43";

/// Cap on bytes read from a synchronous exec's output stream, applied while
/// reading rather than after.
///
/// A margin over `sandbox::EXEC_MAX_OUTPUT_BYTES` (which trims the *response*
/// text) so the raw, still-demuxed stream — 8 bytes of Docker framing per
/// chunk, and both stdout+stderr interleaved — isn't clipped mid-frame before
/// the caller-facing truncation gets a chance to run on the demuxed result.
const EXEC_STREAM_CAP_BYTES: usize = 512 * 1024;

#[derive(Clone)]
/// A client for the Docker Engine API over its unix socket.
pub struct Docker {
    socket: PathBuf,
    client: Client<UnixConnector, Full<Bytes>>,
}

impl Docker {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        let socket = socket.into();
        let client = Client::builder(TokioExecutor::new()).build(UnixConnector {
            socket: socket.clone(),
        });
        Self { socket, client }
    }

    /// Issue one request against the Engine API.
    ///
    /// Returns the parsed body on 2xx. A non-2xx is an error carrying Docker's
    /// own `message`, because the daemon's wording ("no such image", "port is
    /// already allocated") is more useful to a caller than anything this layer
    /// could invent.
    async fn request(
        &self,
        method: hyper::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let uri = hyperlocal_uri(&self.socket, &format!("/{API_VERSION}{path}"))
            .ok_or_else(|| format!("could not build a docker request URI for {path}"))?;

        let (payload, content_type) = match body {
            Some(v) => (
                Full::new(Bytes::from(v.to_string())),
                Some("application/json"),
            ),
            None => (Full::new(Bytes::new()), None),
        };

        let mut builder = hyper::Request::builder().method(method).uri(uri);
        if let Some(ct) = content_type {
            builder = builder.header(hyper::header::CONTENT_TYPE, ct);
        }
        let req = builder
            .body(payload)
            .map_err(|e| format!("could not build the docker request: {e}"))?;

        let resp = self
            .client
            .request(req)
            .await
            .map_err(|e| format!("could not reach the docker daemon: {e}"))?;

        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("could not read the docker response: {e}"))?
            .to_bytes();

        if !status.is_success() {
            let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
            return Err(format!("docker returned {status}: {detail}"));
        }

        if bytes.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| format!("could not parse the docker response: {e}"))
    }

    pub async fn ping(&self) -> Result<(), String> {
        self.request(hyper::Method::GET, "/_ping", None)
            .await
            .map(|_| ())
            // /_ping returns the bare string "OK", which is not JSON. A parse
            // failure here still proves the daemon answered.
            .or_else(|e| {
                if e.contains("could not parse") {
                    Ok(())
                } else {
                    Err(e)
                }
            })
    }

    /// Create a named data volume if needed, then verify its ownership labels.
    ///
    /// Docker treats `POST /volumes/create` as idempotent by name, including
    /// when two create requests race. It does not replace labels on an
    /// existing volume, so the follow-up inspect is the security check: a
    /// volume with the expected name but different labels is never mounted.
    pub async fn ensure_volume(
        &self,
        name: &str,
        labels: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), String> {
        self.request(
            hyper::Method::POST,
            "/volumes/create",
            Some(serde_json::json!({
                "Name": name,
                "Labels": labels,
            })),
        )
        .await?;

        let inspected = self
            .request(hyper::Method::GET, &format!("/volumes/{name}"), None)
            .await?;
        validate_volume_labels(name, &inspected, labels)
    }

    pub async fn create_container(
        &self,
        name: &str,
        spec: serde_json::Value,
    ) -> Result<String, String> {
        let created = self
            .request(
                hyper::Method::POST,
                &format!("/containers/create?name={name}"),
                Some(spec),
            )
            .await?;
        created
            .get("Id")
            .and_then(|id| id.as_str())
            .map(str::to_string)
            .ok_or_else(|| "docker did not return a container id".to_string())
    }

    pub async fn start_container(&self, id: &str) -> Result<(), String> {
        self.request(
            hyper::Method::POST,
            &format!("/containers/{id}/start"),
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn inspect_container(&self, id: &str) -> Result<serde_json::Value, String> {
        self.request(hyper::Method::GET, &format!("/containers/{id}/json"), None)
            .await
    }

    /// Force-remove a container and its anonymous volumes. Docker deliberately
    /// retains named volumes even with `v=true`, so an agent's persistent home
    /// and workspace survive stop and expiry. Removing something already gone
    /// is success: the caller's intent is satisfied, and a reaper racing a
    /// manual `docker rm` should not report failure.
    pub async fn remove_container(&self, id: &str) -> Result<(), String> {
        // Ask the container to stop first so Chromium can flush its persistent
        // SQLite/profile state. Docker applies the container's StopTimeout and
        // kills only after the grace period; deletion remains the final cleanup.
        let _ = self
            .request(hyper::Method::POST, &stop_container_path(id), None)
            .await;
        match self
            .request(hyper::Method::DELETE, &remove_container_path(id), None)
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.contains("404") || e.to_lowercase().contains("no such container") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// List containers carrying the broker's management label.
    ///
    /// Scoped by label so the broker can never see, report on, or reap a
    /// container it did not create — the 83 production containers on this host
    /// are invisible to it.
    pub async fn list_managed(&self, label: &str) -> Result<Vec<serde_json::Value>, String> {
        let filters = serde_json::json!({ "label": [label] }).to_string();
        let encoded = urlencode(&filters);
        let listed = self
            .request(
                hyper::Method::GET,
                &format!("/containers/json?all=true&filters={encoded}"),
                None,
            )
            .await?;
        Ok(listed.as_array().cloned().unwrap_or_default())
    }

    /// Run `argv` inside `id` as `user` (e.g. `"10001:10001"`) and return
    /// `(exit_code, stdout+stderr)`.
    ///
    /// Always an argv vector, never a shell string: `sandbox.rs`'s file routes
    /// build commands from caller-controlled paths, and a shell would let a
    /// crafted path break out of its argument.
    ///
    /// The output stream is Docker's own multiplexed stdout/stderr framing
    /// (8-byte header per chunk); this demultiplexes and concatenates both
    /// streams, which is enough for exec/list operations that don't need to
    /// tell them apart.
    pub async fn exec(
        &self,
        id: &str,
        argv: &[&str],
        user: &str,
    ) -> Result<(i64, Vec<u8>), String> {
        self.exec_with_env(id, argv, user, &[]).await
    }

    /// Like [`Docker::exec`], but with additional `KEY=value` environment
    /// entries for the exec'd process — used to hand a caller-controlled
    /// string (a filesystem path) to an in-container script without ever
    /// interpolating it into that script's source text.
    pub async fn exec_with_env(
        &self,
        id: &str,
        argv: &[&str],
        user: &str,
        env: &[String],
    ) -> Result<(i64, Vec<u8>), String> {
        let create = self
            .request(
                hyper::Method::POST,
                &format!("/containers/{id}/exec"),
                Some(serde_json::json!({
                    "AttachStdout": true,
                    "AttachStderr": true,
                    "Cmd": argv,
                    "User": user,
                    "Env": env,
                })),
            )
            .await?;
        let exec_id = create
            .get("Id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "docker did not return an exec id".to_string())?;

        // Bounded, not `exec_start_raw`'s `.collect()`: a caller-controlled
        // command (this is the primitive behind `POST .../exec`) can emit
        // output far faster than any per-request timeout bounds bytes, and
        // buffering it all before truncating would let one exec drive the
        // broker's own memory up before the caller-facing 256 KiB cap ever
        // applies. `EXEC_STREAM_CAP_BYTES` stops accumulating (with a small
        // margin over the response-level cap) rather than reading to
        // completion first.
        let output = self
            .exec_start_bounded(
                exec_id,
                &serde_json::json!({ "Detach": false, "Tty": false }),
                EXEC_STREAM_CAP_BYTES,
            )
            .await?;

        let inspect = self
            .request(hyper::Method::GET, &format!("/exec/{exec_id}/json"), None)
            .await?;
        let exit_code = inspect
            .get("ExitCode")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);

        Ok((exit_code, demux_docker_stream(&output)))
    }

    /// Start `argv` inside `id` as `user` with `env`, detached: the exec is
    /// created and started with `Detach: true`, so Docker starts the process
    /// and returns immediately rather than streaming its output — this
    /// returns as soon as the process is launched, not when it exits.
    ///
    /// For long-running foreground processes (a GUI app under a window
    /// manager, here) rather than the short commands [`Docker::exec`] and
    /// [`Docker::exec_with_env`] run to completion and collect output from.
    /// Those two would hang for as long as the launched process runs, since
    /// their `Detach: false` start blocks until the stream closes.
    pub async fn exec_detached(
        &self,
        id: &str,
        argv: &[&str],
        user: &str,
        env: &[String],
    ) -> Result<(), String> {
        let create = self
            .request(
                hyper::Method::POST,
                &format!("/containers/{id}/exec"),
                Some(serde_json::json!({
                    "AttachStdout": false,
                    "AttachStderr": false,
                    "Cmd": argv,
                    "User": user,
                    "Env": env,
                })),
            )
            .await?;
        let exec_id = create
            .get("Id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "docker did not return an exec id".to_string())?;

        self.exec_start_raw(
            exec_id,
            &serde_json::json!({ "Detach": true, "Tty": false }),
        )
        .await?;
        Ok(())
    }

    /// `POST /exec/{id}/start`, returning the raw (possibly multiplexed) body
    /// rather than parsing it as JSON — exec output is a byte stream, not a
    /// Docker Engine JSON document.
    async fn exec_start_raw(
        &self,
        exec_id: &str,
        body: &serde_json::Value,
    ) -> Result<Vec<u8>, String> {
        let uri = hyperlocal_uri(
            &self.socket,
            &format!("/{API_VERSION}/exec/{exec_id}/start"),
        )
        .ok_or_else(|| "could not build a docker request URI for exec/start".to_string())?;
        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .map_err(|e| format!("could not build the docker request: {e}"))?;

        let resp = self
            .client
            .request(req)
            .await
            .map_err(|e| format!("could not reach the docker daemon: {e}"))?;
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("could not read the docker response: {e}"))?
            .to_bytes();
        if !status.is_success() {
            let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
            return Err(format!("docker returned {status}: {detail}"));
        }
        Ok(bytes.to_vec())
    }

    /// Like [`Docker::exec_start_raw`], but reads the response body frame by
    /// frame and stops accumulating once `cap_bytes` is reached instead of
    /// collecting the whole stream first.
    ///
    /// The exec still runs to completion on the daemon side either way —
    /// dropping the response body early does not cancel it — this only
    /// bounds how much of its output the broker holds in memory, which is
    /// the property that matters for a process this service cannot itself
    /// throttle beyond the `timeout` wrapper and outer request timeout.
    async fn exec_start_bounded(
        &self,
        exec_id: &str,
        body: &serde_json::Value,
        cap_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        let uri = hyperlocal_uri(
            &self.socket,
            &format!("/{API_VERSION}/exec/{exec_id}/start"),
        )
        .ok_or_else(|| "could not build a docker request URI for exec/start".to_string())?;
        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body.to_string())))
            .map_err(|e| format!("could not build the docker request: {e}"))?;

        let resp = self
            .client
            .request(req)
            .await
            .map_err(|e| format!("could not reach the docker daemon: {e}"))?;
        let status = resp.status();
        let out = read_body_bounded(resp.into_body(), cap_bytes)
            .await
            .map_err(|e| format!("could not read the docker response: {e}"))?;

        if !status.is_success() {
            let detail = serde_json::from_slice::<serde_json::Value>(&out)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&out).trim().to_string());
            return Err(format!("docker returned {status}: {detail}"));
        }
        Ok(out)
    }

    /// Fetch a tar stream of the single path `path` inside `id`.
    ///
    /// Docker's archive endpoint always returns a tar even for one file — the
    /// caller (`main.rs`) unpacks the single entry it expects.
    pub async fn get_archive(&self, id: &str, path: &str) -> Result<Vec<u8>, String> {
        let encoded = urlencode(path);
        let uri = hyperlocal_uri(
            &self.socket,
            &format!("/{API_VERSION}/containers/{id}/archive?path={encoded}"),
        )
        .ok_or_else(|| "could not build a docker request URI for archive".to_string())?;
        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .body(Full::new(Bytes::new()))
            .map_err(|e| format!("could not build the docker request: {e}"))?;
        let resp = self
            .client
            .request(req)
            .await
            .map_err(|e| format!("could not reach the docker daemon: {e}"))?;
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("could not read the docker response: {e}"))?
            .to_bytes();
        if !status.is_success() {
            let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
            return Err(format!("docker returned {status}: {detail}"));
        }
        Ok(bytes.to_vec())
    }

    /// Upload a tar stream (built by the caller with [`tar`]) into `id` at
    /// `dir` — the directory the tar's paths are relative to.
    pub async fn put_archive(&self, id: &str, dir: &str, tar_bytes: Vec<u8>) -> Result<(), String> {
        let encoded = urlencode(dir);
        let uri = hyperlocal_uri(
            &self.socket,
            &format!("/{API_VERSION}/containers/{id}/archive?path={encoded}"),
        )
        .ok_or_else(|| "could not build a docker request URI for archive".to_string())?;
        let req = hyper::Request::builder()
            .method(hyper::Method::PUT)
            .uri(uri)
            .header(hyper::header::CONTENT_TYPE, "application/x-tar")
            .body(Full::new(Bytes::from(tar_bytes)))
            .map_err(|e| format!("could not build the docker request: {e}"))?;
        let resp = self
            .client
            .request(req)
            .await
            .map_err(|e| format!("could not reach the docker daemon: {e}"))?;
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| format!("could not read the docker response: {e}"))?
            .to_bytes();
        if !status.is_success() {
            let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).trim().to_string());
            return Err(format!("docker returned {status}: {detail}"));
        }
        Ok(())
    }
}

fn remove_container_path(id: &str) -> String {
    format!("/containers/{id}?force=true&v=true")
}

fn stop_container_path(id: &str) -> String {
    format!("/containers/{id}/stop?t=60")
}

fn validate_volume_labels(
    name: &str,
    inspected: &serde_json::Value,
    expected: &std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    if inspected.get("Driver").and_then(|value| value.as_str()) != Some("local") {
        return Err(format!(
            "refusing persistent volume {name}: expected Docker local volume driver"
        ));
    }
    if inspected
        .get("Options")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|options| !options.is_empty())
    {
        return Err(format!(
            "refusing persistent volume {name}: driver options could map host storage"
        ));
    }
    let actual = inspected
        .get("Labels")
        .and_then(serde_json::Value::as_object);
    let mismatch = expected.iter().find(|(key, value)| {
        actual
            .and_then(|labels| labels.get(*key))
            .and_then(|v| v.as_str())
            != Some(value.as_str())
    });
    if let Some((key, _)) = mismatch {
        return Err(format!(
            "refusing persistent volume {name}: ownership label {key} is missing or mismatched"
        ));
    }
    Ok(())
}

/// Read a hyper body frame by frame, accumulating at most `cap_bytes`.
///
/// Once the cap is reached the accumulated buffer is truncated to exactly
/// `cap_bytes` and reading stops — later frames are never polled, so nothing
/// past the cap is held in memory even momentarily. Isolated from
/// [`Docker::exec_start_bounded`] so the accumulate-and-stop logic can be
/// exercised directly against a synthetic body in tests, without a live
/// Docker daemon to drive real HTTP framing.
async fn read_body_bounded<B>(mut body: B, cap_bytes: usize) -> Result<Vec<u8>, B::Error>
where
    B: hyper::body::Body + Unpin,
    B::Data: hyper::body::Buf,
{
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame?;
        if let Some(data) = frame.data_ref() {
            out.extend_from_slice(data.chunk());
            if out.len() >= cap_bytes {
                out.truncate(cap_bytes);
                break;
            }
        }
    }
    Ok(out)
}

/// Demultiplex Docker's exec/attach stream framing: each chunk is an 8-byte
/// header (`[stream_type, 0, 0, 0, size_be_u32...]`) followed by `size` bytes
/// of payload. Stdout and stderr are concatenated in stream order — exec
/// output for a file-listing or delete command has no need to tell them apart.
///
/// Malformed framing (a truncated header, a size that overruns the buffer)
/// stops demuxing and returns what was parsed so far rather than failing the
/// whole call — Docker's raw stream should always be well-formed, but a
/// partial parse is more useful than none if it ever is not.
fn demux_docker_stream(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i + 8 <= raw.len() {
        let size = u32::from_be_bytes([raw[i + 4], raw[i + 5], raw[i + 6], raw[i + 7]]) as usize;
        let start = i + 8;
        let end = start + size;
        if end > raw.len() {
            break;
        }
        out.extend_from_slice(&raw[start..end]);
        i = end;
    }
    out
}

/// Percent-encode the bytes that matter inside a query string value.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Encode a unix socket path into the `unix://<hex>/path` URI hyper's
/// connector expects.
///
/// Returns `None` rather than panicking: the host component is hex so it is
/// always URI-safe, but `path` is composed from an API version and a caller's
/// container id, and a malformed request should surface as a request error.
fn hyperlocal_uri(socket: &std::path::Path, path: &str) -> Option<hyper::Uri> {
    let host = hex::encode(socket.to_string_lossy().as_bytes());
    format!("unix://{host}{path}").parse().ok()
}

/// Connector that dials a unix socket, decoding the path from the URI host.
#[derive(Clone)]
/// Connector that dials the Docker socket, decoding its path from the URI host.
pub struct UnixConnector {
    socket: PathBuf,
}

impl tower_service::Service<hyper::Uri> for UnixConnector {
    type Response = hyper_util::rt::TokioIo<tokio::net::UnixStream>;
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: hyper::Uri) -> Self::Future {
        let socket = self.socket.clone();
        Box::pin(async move {
            let stream = tokio::net::UnixStream::connect(socket).await?;
            Ok(hyper_util::rt::TokioIo::new(stream))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_json_punctuation() {
        // Docker's filters parameter is JSON in a query string; unescaped
        // braces and quotes would be rejected or silently truncated.
        let encoded = urlencode(r#"{"label":["a=b"]}"#);
        assert!(!encoded.contains('{'));
        assert!(!encoded.contains('"'));
        assert!(encoded.contains("%7B"));
    }

    #[test]
    fn urlencode_leaves_unreserved_characters_alone() {
        assert_eq!(urlencode("abcXYZ019-_.~"), "abcXYZ019-_.~");
    }

    #[test]
    fn volume_labels_accept_exact_or_additional_metadata() {
        let expected = std::collections::BTreeMap::from([
            ("com.buzz.sandbox.volume".to_string(), "1".to_string()),
            (
                "com.buzz.sandbox.volume-role".to_string(),
                "home".to_string(),
            ),
        ]);
        assert!(validate_volume_labels(
            "buzz-pc-safe-home",
            &serde_json::json!({"Labels": {
                "com.buzz.sandbox.volume": "1",
                "com.buzz.sandbox.volume-role": "home",
                "operator.note": "retained"
            }, "Driver": "local", "Options": {}}),
            &expected,
        )
        .is_ok());
    }

    #[test]
    fn volume_labels_refuse_missing_or_mismatched_ownership() {
        let expected = std::collections::BTreeMap::from([(
            "com.buzz.sandbox.storage-id".to_string(),
            "secret-expected-value".to_string(),
        )]);
        for inspected in [
            serde_json::json!({"Labels": {}, "Driver": "local"}),
            serde_json::json!({"Labels": {"com.buzz.sandbox.storage-id": "other"}, "Driver": "local"}),
            serde_json::json!({"Driver": "local"}),
        ] {
            let error = validate_volume_labels("buzz-pc-safe-home", &inspected, &expected)
                .expect_err("unowned volume must never be mounted");
            assert!(error.contains("com.buzz.sandbox.storage-id"));
            assert!(!error.contains("secret-expected-value"));
        }
    }

    #[test]
    fn removing_compute_requests_anonymous_cleanup_but_keeps_named_volumes() {
        // Docker's `v=true` removes anonymous volumes only. The container spec
        // uses explicitly named volumes, so this exact request preserves PC data.
        assert_eq!(
            stop_container_path("abc123"),
            "/containers/abc123/stop?t=60"
        );
        assert_eq!(
            remove_container_path("abc123"),
            "/containers/abc123?force=true&v=true"
        );
    }

    #[test]
    fn volume_validation_refuses_nonlocal_or_optioned_drivers() {
        let labels = std::collections::BTreeMap::new();
        assert!(validate_volume_labels(
            "volume",
            &serde_json::json!({"Driver": "nfs", "Labels": {}}),
            &labels,
        )
        .is_err());
        assert!(validate_volume_labels(
            "volume",
            &serde_json::json!({
                "Driver": "local",
                "Options": {"type": "none", "o": "bind", "device": "/host"},
                "Labels": {}
            }),
            &labels,
        )
        .is_err());
    }

    #[test]
    fn socket_path_round_trips_through_the_uri_host() {
        let uri = hyperlocal_uri(std::path::Path::new("/var/run/docker.sock"), "/v1.43/_ping")
            .expect("a well-formed path yields a URI");
        let host = uri.host().expect("uri has a host");
        let decoded = String::from_utf8(hex::decode(host).expect("host is hex")).expect("utf8");
        assert_eq!(decoded, "/var/run/docker.sock");
        assert_eq!(uri.path(), "/v1.43/_ping");
    }

    /// A body that yields a fixed queue of data frames, one per `frame()`
    /// poll — enough to drive [`read_body_bounded`] without a live Docker
    /// daemon or connection.
    struct FixedFrames(std::collections::VecDeque<Bytes>);

    impl hyper::body::Body for FixedFrames {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
            std::task::Poll::Ready(self.0.pop_front().map(|b| Ok(hyper::body::Frame::data(b))))
        }
    }

    #[tokio::test]
    async fn read_body_bounded_returns_everything_under_the_cap() {
        let body =
            FixedFrames(vec![Bytes::from_static(b"hello "), Bytes::from_static(b"world")].into());
        let out = read_body_bounded(body, 1024).await.unwrap();
        assert_eq!(out, b"hello world");
    }

    /// The load-bearing case: a body that produces far more than the cap must
    /// never accumulate past it, regardless of how many frames remain
    /// unread — this is what stops a runaway exec (`yes | head -c 5G`, say)
    /// from driving the broker's own memory up before the caller-facing
    /// truncation in `main.rs` ever runs.
    #[tokio::test]
    async fn read_body_bounded_stops_accumulating_at_the_cap() {
        let frames: std::collections::VecDeque<Bytes> = (0..10_000)
            .map(|_| Bytes::from_static(b"0123456789"))
            .collect();
        let body = FixedFrames(frames);
        let out = read_body_bounded(body, 55).await.unwrap();
        assert_eq!(out.len(), 55, "output must be truncated to exactly the cap");
    }

    #[tokio::test]
    async fn read_body_bounded_handles_an_empty_body() {
        let body = FixedFrames(std::collections::VecDeque::new());
        let out = read_body_bounded(body, 1024).await.unwrap();
        assert!(out.is_empty());
    }
}
