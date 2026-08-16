//! Driving a tool server that runs on another machine.
//!
//! `mcp.rs` spawns tool servers as child processes and speaks MCP over the
//! pipe. That is right when the tools belong to the same machine as the model,
//! and it is the default.
//!
//! It is wrong when the tools are a sandbox: a disposable container with a
//! shell, a filesystem, and a browser, deliberately holding no model credential
//! of its own. There the agent's reasoning stays here and only the *hands* live
//! over there, so the transport has to cross a network.
//!
//! # Why this is not just a URL
//!
//! A tool endpoint that runs shell commands is remote code execution by
//! design, so the sandbox authenticates every call by Buzz identity (NIP-98)
//! and serves exactly one owner. Meeting that requires a signature that covers
//! **this request's body**, since the signature is what proves which tool was
//! asked for with which arguments.
//!
//! `rmcp`'s transport config carries a single static `auth_header`, applied to
//! every request and sent as `Bearer`. Neither fits: the value must change per
//! request, and the scheme is `Nostr`. So this module implements
//! `StreamableHttpClient` itself — a thin wrapper that delegates the HTTP work
//! to `reqwest` and adds one thing, a freshly signed `Authorization` header
//! built from the exact bytes about to be sent.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::BoxStream;
use futures::StreamExt as _;
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};

use crate::types::AgentError;

/// A `reqwest` client that signs each request with the agent's Buzz identity.
///
/// Cloned per request by `rmcp`, so the key is held behind an `Arc` and signing
/// is cheap: one event built, signed, and base64-encoded per call.
#[derive(Clone)]
pub struct NostrSigningClient {
    http: reqwest::Client,
    keys: Arc<nostr::Keys>,
}

impl NostrSigningClient {
    /// `nsec` is the agent's own key — the same identity the sandbox was told
    /// to accept as its owner.
    pub fn new(nsec: &str) -> Result<Self, AgentError> {
        let keys = nostr::Keys::parse(nsec.trim()).map_err(|e| {
            AgentError::Mcp(format!(
                "remote tools need the agent's key to sign requests, but it is not \
                 a valid key: {e}"
            ))
        })?;
        Ok(Self {
            http: reqwest::Client::new(),
            keys: Arc::new(keys),
        })
    }

    /// Build a NIP-98 `Authorization` value for one request.
    ///
    /// The event commits to the method, the full URL, and — when there is one —
    /// a hash of the body. A server that verifies it therefore knows the exact
    /// tool call it is being asked to run has not been altered in transit.
    fn sign(&self, method: &str, url: &str, body: &[u8]) -> Result<String, AgentError> {
        use base64::Engine;
        use sha2::{Digest, Sha256};

        let mut tags = vec![
            nostr::Tag::parse(["u", url])
                .map_err(|e| AgentError::Mcp(format!("could not build the auth event: {e}")))?,
            nostr::Tag::parse(["method", method])
                .map_err(|e| AgentError::Mcp(format!("could not build the auth event: {e}")))?,
        ];
        if !body.is_empty() {
            let hash = hex::encode(Sha256::digest(body));
            tags.push(
                nostr::Tag::parse(["payload", &hash])
                    .map_err(|e| AgentError::Mcp(format!("could not build the auth event: {e}")))?,
            );
        }

        let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .map_err(|e| AgentError::Mcp(format!("could not sign the tool request: {e}")))?;
        let json = serde_json::to_string(&event)
            .map_err(|e| AgentError::Mcp(format!("could not serialize the auth event: {e}")))?;
        Ok(format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(json)
        ))
    }
}

impl StreamableHttpClient for NostrSigningClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_token: Option<String>,
        _custom_headers: HashMap<http::HeaderName, http::HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        // Serialize once and send those exact bytes: signing a re-serialization
        // would risk a different byte sequence than the one hashed, and the
        // server would reject a signature that is in fact correct.
        let body = serde_json::to_vec(&message).map_err(|e| {
            StreamableHttpError::UnexpectedServerResponse(
                format!("could not serialize the request: {e}").into(),
            )
        })?;

        let auth = self
            .sign("POST", uri.as_ref(), &body)
            .map_err(|e| StreamableHttpError::UnexpectedServerResponse(e.to_string().into()))?;

        let mut request = self
            .http
            .post(uri.as_ref())
            .header(
                reqwest::header::ACCEPT,
                [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "),
            )
            .header(reqwest::header::CONTENT_TYPE, JSON_MIME_TYPE)
            .header(reqwest::header::AUTHORIZATION, auth);

        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }

        let response = request.body(body).send().await?;
        let status = response.status();

        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        // Only meaningful once a session exists; otherwise a 404 is a bad URL.
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        // Surface the sandbox's own wording ("this sandbox is not owned by that
        // identity") rather than a bare status, since that names the fix.
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            let detail = response.text().await.unwrap_or_default();
            return Err(StreamableHttpError::UnexpectedServerResponse(
                format!("the sandbox refused this request ({status}): {detail}").into(),
            ));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let content_length = response.content_length();

        // Some servers answer a notification with an empty 200 rather than the
        // 202 the spec asks for; treat that as accepted.
        if status.is_success()
            && content_length == Some(0)
            && matches!(
                message,
                ClientJsonRpcMessage::Notification(_)
                    | ClientJsonRpcMessage::Response(_)
                    | ClientJsonRpcMessage::Error(_)
            )
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }

        let response = response.error_for_status()?;
        match content_type {
            Some(ct) if ct.starts_with(EVENT_STREAM_MIME_TYPE) => {
                let stream = SseStream::from_byte_stream(response.bytes_stream()).boxed();
                Ok(StreamableHttpPostResponse::Sse(stream, session_id))
            }
            Some(ct) if ct.starts_with(JSON_MIME_TYPE) => {
                let message: rmcp::model::ServerJsonRpcMessage = response.json().await?;
                Ok(StreamableHttpPostResponse::Json(message, session_id))
            }
            other => Err(StreamableHttpError::UnexpectedContentType(other)),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_token: Option<String>,
        _custom_headers: HashMap<http::HeaderName, http::HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let auth = self
            .sign("DELETE", uri.as_ref(), &[])
            .map_err(|e| StreamableHttpError::UnexpectedServerResponse(e.to_string().into()))?;
        let response = self
            .http
            .delete(uri.as_ref())
            .header(HEADER_SESSION_ID, session_id.as_ref())
            .header(reqwest::header::AUTHORIZATION, auth)
            .send()
            .await?;
        // Session deletion is optional in the spec; a server that does not
        // support it is not an error worth failing a shutdown over.
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response.error_for_status()?;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_token: Option<String>,
        _custom_headers: HashMap<http::HeaderName, http::HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, StreamableHttpError<Self::Error>>
    {
        let auth = self
            .sign("GET", uri.as_ref(), &[])
            .map_err(|e| StreamableHttpError::UnexpectedServerResponse(e.to_string().into()))?;
        let mut request = self
            .http
            .get(uri.as_ref())
            .header(reqwest::header::ACCEPT, EVENT_STREAM_MIME_TYPE)
            .header(HEADER_SESSION_ID, session_id.as_ref())
            .header(reqwest::header::AUTHORIZATION, auth);
        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }

        let response = request.send().await?;
        // A server with nothing to stream says so; that is not a failure.
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response.error_for_status()?;
        match response.headers().get(reqwest::header::CONTENT_TYPE) {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                Ok(SseStream::from_byte_stream(response.bytes_stream()).boxed())
            }
            other => Err(StreamableHttpError::UnexpectedContentType(
                other.map(|v| String::from_utf8_lossy(v.as_bytes()).to_string()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> NostrSigningClient {
        let keys = nostr::Keys::generate();
        NostrSigningClient::new(&keys.secret_key().to_secret_hex()).expect("client")
    }

    #[test]
    fn refuses_a_key_it_cannot_parse() {
        let err = match NostrSigningClient::new("not-a-key") {
            Ok(_) => panic!("must refuse an unparseable key"),
            Err(e) => e,
        };
        assert!(
            format!("{err}").contains("not a valid key"),
            "unexpected: {err}"
        );
    }

    /// The header must be a NIP-98 event, not a bearer token — the sandbox
    /// rejects anything else.
    #[test]
    fn signs_with_the_nostr_scheme_and_a_verifiable_event() {
        use base64::Engine;
        let c = client();
        let header = c
            .sign("POST", "http://sandbox:9320/mcp", b"body")
            .expect("sign");
        let encoded = header.strip_prefix("Nostr ").expect("Nostr scheme");
        let json = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("base64"),
        )
        .expect("utf8");
        let event: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(event["kind"], 27235, "must be a NIP-98 HTTP-auth event");
    }

    /// The body hash is what stops a tool call being swapped in transit, so a
    /// different body must produce a different signature.
    #[test]
    fn a_different_body_produces_a_different_signature() {
        let c = client();
        let a = c.sign("POST", "http://s/mcp", b"call-a").expect("sign a");
        let b = c.sign("POST", "http://s/mcp", b"call-b").expect("sign b");
        assert_ne!(a, b);
    }

    /// A signature for one URL must not be reusable against another.
    #[test]
    fn a_different_url_produces_a_different_signature() {
        let c = client();
        let a = c.sign("POST", "http://a/mcp", b"x").expect("sign a");
        let b = c.sign("POST", "http://b/mcp", b"x").expect("sign b");
        assert_ne!(a, b);
    }

    /// An empty body (GET/DELETE) must omit the payload tag rather than hash
    /// nothing, matching what the verifier expects.
    #[test]
    fn omits_the_payload_tag_when_there_is_no_body() {
        use base64::Engine;
        let c = client();
        let header = c.sign("GET", "http://s/mcp", b"").expect("sign");
        let json = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(header.strip_prefix("Nostr ").expect("scheme"))
                .expect("base64"),
        )
        .expect("utf8");
        let event: serde_json::Value = serde_json::from_str(&json).expect("json");
        let tags = event["tags"].as_array().expect("tags");
        assert!(
            !tags.iter().any(|t| t[0] == "payload"),
            "empty body must not carry a payload tag: {tags:?}"
        );
    }

    /// The signature the sandbox will actually check — verified with the same
    /// function the server uses, so this proves interoperability rather than
    /// merely that the two sides agree with themselves.
    #[test]
    fn produces_a_signature_the_server_side_verifier_accepts() {
        use base64::Engine;
        let keys = nostr::Keys::generate();
        let c = NostrSigningClient::new(&keys.secret_key().to_secret_hex()).expect("client");

        let url = "http://172.16.240.2:9320/mcp";
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let header = c.sign("POST", url, body).expect("sign");
        let json = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(header.strip_prefix("Nostr ").expect("scheme"))
                .expect("base64"),
        )
        .expect("utf8");

        let verified = buzz_auth::verify_nip98_event(&json, url, "POST", Some(body))
            .expect("the server-side verifier must accept this signature");
        assert_eq!(verified.to_hex(), keys.public_key().to_hex());
    }
}
