//! A transparent reverse proxy to a sandbox's in-container noVNC server.
//!
//! The sandbox desktop is served by websockify on port 6080: it hosts the
//! static noVNC client over HTTP *and* the RFB stream over a WebSocket on the
//! same port. This proxy forwards both from a single handler — plain GETs for
//! the client assets stream straight through, and the WebSocket upgrade is
//! tunnelled byte-for-byte — so the browser reaches a container that publishes
//! no host port and lives on an isolated network the browser cannot route to.
//!
//! Authentication happens before this is ever called (see the desktop route in
//! `main.rs`): a caller without a valid signed token never reaches the proxy.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use hyper::upgrade::OnUpgrade;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::sync::OnceLock;
use tokio::io::copy_bidirectional;

fn client() -> &'static Client<HttpConnector, Body> {
    static CLIENT: OnceLock<Client<HttpConnector, Body>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut connector = HttpConnector::new();
        // Interactive stream: don't buffer small RFB frames.
        connector.set_nodelay(true);
        // websockify speaks HTTP/1 only.
        Client::builder(TokioExecutor::new()).build(connector)
    })
}

/// Forward `req` to `http://{authority}/{rest}`, tunnelling a WebSocket upgrade
/// when present. `authority` is `ip:6080`; `rest` is the path beyond the
/// sandbox's desktop prefix (empty for the client's index).
pub async fn desktop_proxy(authority: &str, rest: &str, mut req: Request) -> Response {
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let new_uri = match format!("http://{authority}/{rest}{query}").parse() {
        Ok(uri) => uri,
        Err(_) => return (StatusCode::BAD_GATEWAY, "bad upstream uri").into_response(),
    };
    *req.uri_mut() = new_uri;

    let is_upgrade = is_websocket_upgrade(&req);

    // Take the client-facing upgrade future BEFORE the request is consumed by
    // the send below. It resolves only once we return the 101 downstream.
    let client_upgrade: Option<OnUpgrade> = if is_upgrade {
        Some(hyper::upgrade::on(&mut req))
    } else {
        None
    };

    if let Ok(host) = HeaderValue::from_str(authority) {
        req.headers_mut().insert(header::HOST, host);
    }
    strip_hop_by_hop(req.headers_mut(), is_upgrade);

    let upstream = match client().request(req).await {
        Ok(resp) => resp,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("upstream error: {e}")).into_response();
        }
    };

    // Plain HTTP (or a non-101 response to an upgrade attempt): stream it back.
    if !is_upgrade || upstream.status() != StatusCode::SWITCHING_PROTOCOLS {
        let (parts, incoming) = upstream.into_parts();
        return Response::from_parts(parts, Body::new(incoming));
    }

    // WebSocket: tunnel bytes both ways once each side finishes its handshake.
    let mut upstream = upstream;
    let upstream_upgrade = hyper::upgrade::on(&mut upstream);
    let client_upgrade = match client_upgrade {
        Some(u) => u,
        None => return (StatusCode::BAD_GATEWAY, "missing client upgrade").into_response(),
    };
    tokio::spawn(async move {
        match tokio::try_join!(client_upgrade, upstream_upgrade) {
            Ok((client_io, upstream_io)) => {
                let mut a = TokioIo::new(client_io);
                let mut b = TokioIo::new(upstream_io);
                if let Err(e) = copy_bidirectional(&mut a, &mut b).await {
                    tracing::debug!(error = %e, "sandbox desktop ws closed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "sandbox desktop ws upgrade failed"),
        }
    });

    // Hand the upstream's 101 (with its Upgrade/Sec-WebSocket-Accept headers)
    // back verbatim so the client completes its side of the handshake.
    let (parts, _incoming) = upstream.into_parts();
    Response::from_parts(parts, Body::empty())
}

fn is_websocket_upgrade(req: &Request) -> bool {
    let headers = req.headers();
    let has_upgrade = headers
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false);
    let is_ws = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    has_upgrade && is_ws
}

/// Strip hop-by-hop headers that must not cross a proxy. On an upgrade request
/// the `Connection`/`Upgrade`/`Sec-WebSocket-*` headers are preserved — they are
/// exactly what make the upstream reply 101.
fn strip_hop_by_hop(headers: &mut header::HeaderMap, is_upgrade: bool) {
    headers.remove(header::PROXY_AUTHENTICATE);
    headers.remove(header::PROXY_AUTHORIZATION);
    headers.remove(header::TE);
    headers.remove(header::TRAILER);
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove("keep-alive");
    if !is_upgrade {
        headers.remove(header::CONNECTION);
        headers.remove(header::UPGRADE);
    }
}
