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
use hyper::body::Bytes;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::path::PathBuf;

/// Docker Engine API version to pin. Pinning avoids a daemon upgrade silently
/// changing response shapes under us; 1.43 is available on Docker 24+ and the
/// host runs 29.x.
const API_VERSION: &str = "v1.43";

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

    /// Force-remove a container and its anonymous volumes. Removing something
    /// already gone is success — the caller's intent (it should not exist) is
    /// satisfied either way, and a reaper racing a manual `docker rm` should
    /// not report failure.
    pub async fn remove_container(&self, id: &str) -> Result<(), String> {
        match self
            .request(
                hyper::Method::DELETE,
                &format!("/containers/{id}?force=true&v=true"),
                None,
            )
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
    fn socket_path_round_trips_through_the_uri_host() {
        let uri = hyperlocal_uri(std::path::Path::new("/var/run/docker.sock"), "/v1.43/_ping")
            .expect("a well-formed path yields a URI");
        let host = uri.host().expect("uri has a host");
        let decoded = String::from_utf8(hex::decode(host).expect("host is hex")).expect("utf8");
        assert_eq!(decoded, "/var/run/docker.sock");
        assert_eq!(uri.path(), "/v1.43/_ping");
    }
}
