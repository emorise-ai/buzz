//! Browser tools — drive the agent's own Chromium over the DevTools protocol.
//!
//! These tools operate on the **same browser a human can see and take over**
//! (the desktop image runs Chromium on an X display exported over VNC —
//! `Dockerfile.sprig-desktop`). That is the whole design: the agent browses,
//! and when it hits a login wall or a CAPTCHA, a person opens the viewer, types
//! their own credentials, and the agent carries on with an authenticated
//! session. The credentials go into the browser's password store, never into
//! the model's context.
//!
//! Communication is CDP over a websocket. The HTTP endpoints alone
//! (`/json`, `/json/new`) can list and open tabs but cannot evaluate script,
//! click, or type, so a websocket is unavoidable for anything useful.
//!
//! Every tool fails with a plain, actionable message when no browser is
//! present, because the same MCP server also ships in the non-desktop image
//! where these tools simply cannot work.

use rmcp::model::{CallToolResult, Content, ErrorData};
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;

/// Where Chromium exposes CDP inside the agent's container. Overridable for
/// tests and for a sidecar browser; never a user-supplied per-call value,
/// which would let a prompt point the agent at an arbitrary host.
fn cdp_base() -> String {
    std::env::var("BUZZ_BROWSER_CDP").unwrap_or_else(|_| "http://127.0.0.1:9222".to_string())
}

/// One CDP round trip is bounded: a page that never settles must surface as a
/// tool error the agent can react to, not a hung turn.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Page text is truncated hard. An LLM does not need 2 MB of DOM, and an
/// unbounded return would blow the context window on one call.
const MAX_TEXT: usize = 20_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NavigateParams {
    /// Absolute URL to open, e.g. `https://example.com`.
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// CSS selector of the element to click.
    pub selector: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeParams {
    /// CSS selector of the input to type into.
    pub selector: String,
    /// Text to enter. Never put a password here — a human types those in the
    /// viewer so they stay out of the transcript.
    pub text: String,
    /// Press Enter afterwards. Defaults to false.
    #[serde(default)]
    pub submit: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// Optional CSS selector to scope the text. Omit for the whole page.
    #[serde(default)]
    pub selector: Option<String>,
}

/// A page target, as CDP reports it.
struct Target {
    id: String,
    ws_url: String,
    title: String,
    url: String,
}

async fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(CALL_TIMEOUT)
        .build()
        .unwrap_or_default()
}

fn no_browser(detail: &str) -> ErrorData {
    ErrorData::internal_error(
        format!(
            "no browser is available in this sandbox ({detail}). The desktop \
             image (buzz-sprig-desktop) provides one; a plain agent image does \
             not."
        ),
        None,
    )
}

/// List page targets, newest first — the active tab is the one most recently
/// opened, which is what an agent means by "the page" after a navigate.
async fn targets() -> Result<Vec<Target>, ErrorData> {
    let base = cdp_base();
    let resp = http()
        .await
        .get(format!("{base}/json"))
        .send()
        .await
        .map_err(|e| no_browser(&e.to_string()))?;
    let list: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ErrorData::internal_error(format!("bad CDP response: {e}"), None))?;
    Ok(list
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
        .filter_map(|t| {
            Some(Target {
                id: t.get("id")?.as_str()?.to_string(),
                ws_url: t.get("webSocketDebuggerUrl")?.as_str()?.to_string(),
                title: t
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                url: t
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

async fn active_target() -> Result<Target, ErrorData> {
    targets()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| no_browser("no open pages"))
}

/// Run one CDP method on a target and return its `result`.
///
/// Opens a websocket per call rather than holding one open. A persistent
/// connection would be faster, but the agent's calls are seconds apart and a
/// stale socket across a browser restart is a failure mode that costs more
/// than the handshake saves.
async fn cdp(
    ws_url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ErrorData> {
    use futures_util::{SinkExt, StreamExt};

    let (mut socket, _) =
        tokio::time::timeout(CALL_TIMEOUT, tokio_tungstenite::connect_async(ws_url))
            .await
            .map_err(|_| {
                ErrorData::internal_error("timed out connecting to the browser".to_string(), None)
            })?
            .map_err(|e| no_browser(&e.to_string()))?;

    let msg = serde_json::json!({"id": 1, "method": method, "params": params});
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            msg.to_string().into(),
        ))
        .await
        .map_err(|e| ErrorData::internal_error(format!("browser send failed: {e}"), None))?;

    // CDP interleaves events with responses; read until our id comes back.
    let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(ErrorData::internal_error(
                format!("browser did not answer {method} in time"),
                None,
            ));
        }
        let next = tokio::time::timeout(remaining, socket.next())
            .await
            .map_err(|_| {
                ErrorData::internal_error(format!("browser did not answer {method} in time"), None)
            })?;
        let Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) = next else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_u64()) != Some(1) {
            continue;
        }
        if let Some(err) = value.get("error") {
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(ErrorData::internal_error(
                format!("browser rejected {method}: {message}"),
                None,
            ));
        }
        return Ok(value
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null));
    }
}

/// Evaluate JavaScript in the page and return the value as a string.
async fn eval(ws_url: &str, expression: &str) -> Result<String, ErrorData> {
    let result = cdp(
        ws_url,
        "Runtime.evaluate",
        serde_json::json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
    )
    .await?;

    // A thrown exception is a real failure the agent must see, not an empty
    // string it would silently treat as success.
    if let Some(details) = result.get("exceptionDetails") {
        let text = details
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| details.get("text").and_then(|t| t.as_str()))
            .unwrap_or("script error");
        return Err(ErrorData::internal_error(
            format!("page script failed: {text}"),
            None,
        ));
    }

    Ok(result
        .get("result")
        .and_then(|r| r.get("value"))
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default())
}

/// Escape a string for embedding in a single-quoted JS literal.
fn js_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('\'');
    for c in raw.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_TEXT {
        text.truncate(MAX_TEXT);
        text.push_str("\n… (truncated)");
    }
    text
}

pub async fn navigate(p: NavigateParams) -> Result<CallToolResult, ErrorData> {
    if !p.url.starts_with("http://") && !p.url.starts_with("https://") {
        return Ok(CallToolResult::error(vec![Content::text(
            "url must start with http:// or https://",
        )]));
    }
    let target = active_target().await?;
    cdp(
        &target.ws_url,
        "Page.navigate",
        serde_json::json!({"url": p.url}),
    )
    .await?;

    // Settle briefly so the reported title reflects the new page rather than
    // the old one — the agent's next step usually depends on it.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let title = eval(&target.ws_url, "document.title")
        .await
        .unwrap_or_default();
    let url = eval(&target.ws_url, "location.href")
        .await
        .unwrap_or_default();
    Ok(CallToolResult::success(vec![Content::text(format!(
        "Opened {url}\nTitle: {title}\n\nA human can watch and take over this \
         page in the desktop viewer — use that for logins rather than typing \
         credentials here."
    ))]))
}

pub async fn click(p: ClickParams) -> Result<CallToolResult, ErrorData> {
    let target = active_target().await?;
    let sel = js_string(&p.selector);
    let script = format!(
        "(() => {{ const el = document.querySelector({sel}); \
         if (!el) return 'NOT_FOUND'; el.scrollIntoView({{block:'center'}}); \
         el.click(); return 'OK'; }})()"
    );
    match eval(&target.ws_url, &script).await?.as_str() {
        "NOT_FOUND" => Ok(CallToolResult::error(vec![Content::text(format!(
            "no element matches {:?} on this page",
            p.selector
        ))])),
        _ => {
            tokio::time::sleep(Duration::from_millis(800)).await;
            let url = eval(&target.ws_url, "location.href")
                .await
                .unwrap_or_default();
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Clicked {:?}\nNow at: {url}",
                p.selector
            ))]))
        }
    }
}

pub async fn type_text(p: TypeParams) -> Result<CallToolResult, ErrorData> {
    let target = active_target().await?;
    let sel = js_string(&p.selector);
    let text = js_string(&p.text);
    // Dispatch input+change so frameworks (React et al.) observe the value.
    let script = format!(
        "(() => {{ const el = document.querySelector({sel}); \
         if (!el) return 'NOT_FOUND'; el.focus(); el.value = {text}; \
         el.dispatchEvent(new Event('input', {{bubbles:true}})); \
         el.dispatchEvent(new Event('change', {{bubbles:true}})); \
         return 'OK'; }})()"
    );
    if eval(&target.ws_url, &script).await? == "NOT_FOUND" {
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "no element matches {:?} on this page",
            p.selector
        ))]));
    }

    if p.submit {
        let submit = format!(
            "(() => {{ const el = document.querySelector({sel}); \
             if (el && el.form) {{ el.form.submit(); return 'FORM'; }} \
             return 'KEY'; }})()"
        );
        let how = eval(&target.ws_url, &submit).await.unwrap_or_default();
        if how == "KEY" {
            // No owning form — send a real Enter keypress instead.
            let _ = cdp(
                &target.ws_url,
                "Input.dispatchKeyEvent",
                serde_json::json!({"type":"keyDown","key":"Enter","windowsVirtualKeyCode":13}),
            )
            .await;
            let _ = cdp(
                &target.ws_url,
                "Input.dispatchKeyEvent",
                serde_json::json!({"type":"keyUp","key":"Enter","windowsVirtualKeyCode":13}),
            )
            .await;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }

    let url = eval(&target.ws_url, "location.href")
        .await
        .unwrap_or_default();
    Ok(CallToolResult::success(vec![Content::text(format!(
        "Typed into {:?}{}\nNow at: {url}",
        p.selector,
        if p.submit { " and submitted" } else { "" }
    ))]))
}

pub async fn read_page(p: ReadParams) -> Result<CallToolResult, ErrorData> {
    let target = active_target().await?;
    let script = match p.selector.as_deref() {
        Some(sel) => {
            let sel = js_string(sel);
            format!(
                "(() => {{ const el = document.querySelector({sel}); \
                 return el ? el.innerText : 'NOT_FOUND'; }})()"
            )
        }
        None => "document.body ? document.body.innerText : ''".to_string(),
    };
    let text = eval(&target.ws_url, &script).await?;
    if text == "NOT_FOUND" {
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "no element matches {:?} on this page",
            p.selector.unwrap_or_default()
        ))]));
    }
    let title = eval(&target.ws_url, "document.title")
        .await
        .unwrap_or_default();
    let url = eval(&target.ws_url, "location.href")
        .await
        .unwrap_or_default();
    Ok(CallToolResult::success(vec![Content::text(format!(
        "{title}\n{url}\n\n{}",
        truncate(text)
    ))]))
}

pub async fn tabs() -> Result<CallToolResult, ErrorData> {
    let list = targets().await?;
    if list.is_empty() {
        return Ok(CallToolResult::success(vec![Content::text(
            "no pages are open",
        )]));
    }
    let mut out = String::from("Open pages (newest first):\n");
    for (i, t) in list.iter().enumerate() {
        out.push_str(&format!("{}. {} — {}\n", i + 1, t.title, t.url));
    }
    let _ = &list[0].id;
    Ok(CallToolResult::success(vec![Content::text(out)]))
}

/// Screenshot the current page as an image the model can actually look at.
pub async fn screenshot() -> Result<CallToolResult, ErrorData> {
    let target = active_target().await?;
    let result = cdp(
        &target.ws_url,
        "Page.captureScreenshot",
        serde_json::json!({"format": "png"}),
    )
    .await?;
    let data = result
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| ErrorData::internal_error("browser returned no image".to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::image(
        data.to_string(),
        "image/png".to_string(),
    )]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_strings_are_escaped_so_a_selector_cannot_inject_script() {
        // A selector containing a quote must not terminate the literal — that
        // would let page content or a crafted prompt run arbitrary script.
        let hostile = "'); alert('x";
        let escaped = js_string(hostile);
        assert!(escaped.starts_with('\''));
        assert!(escaped.ends_with('\''));
        // The property that matters: every inner quote is backslash-escaped,
        // so the literal cannot be terminated early. Checking for the absence
        // of the substring would be wrong — it still appears, defanged.
        let inner = &escaped[1..escaped.len() - 1];
        for (i, c) in inner.char_indices() {
            if c == '\'' {
                assert!(
                    i > 0 && inner.as_bytes()[i - 1] == b'\\',
                    "unescaped quote at {i} in {escaped}"
                );
            }
        }
    }

    #[test]
    fn backslashes_and_newlines_survive_escaping() {
        assert_eq!(js_string("a\\b"), "'a\\\\b'");
        assert_eq!(js_string("a\nb"), "'a\\nb'");
        assert_eq!(js_string("plain"), "'plain'");
    }

    #[test]
    fn long_pages_are_truncated_with_a_marker() {
        let long = "x".repeat(MAX_TEXT + 500);
        let out = truncate(long);
        assert!(out.len() < MAX_TEXT + 100);
        assert!(out.ends_with("(truncated)"));
    }

    #[test]
    fn short_pages_pass_through_unchanged() {
        assert_eq!(truncate("hello".into()), "hello");
    }

    #[test]
    fn cdp_base_is_env_controlled_not_caller_controlled() {
        // A per-call host would let a prompt aim the agent's browser tooling at
        // an arbitrary endpoint.
        let before = std::env::var("BUZZ_BROWSER_CDP").ok();
        std::env::remove_var("BUZZ_BROWSER_CDP");
        assert_eq!(cdp_base(), "http://127.0.0.1:9222");
        std::env::set_var("BUZZ_BROWSER_CDP", "http://example:1234");
        assert_eq!(cdp_base(), "http://example:1234");
        match before {
            Some(v) => std::env::set_var("BUZZ_BROWSER_CDP", v),
            None => std::env::remove_var("BUZZ_BROWSER_CDP"),
        }
    }
}
