//! Username/password login through Home Assistant's auth flow, and token
//! refresh. See docs/protocol.md section 1.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::sys;
use serde_json::{json, Value};

pub struct Tokens {
    pub access: String,
    pub refresh: String,
}

/// Outcome of submitting credentials.
pub enum LoginStep {
    /// Logged in.
    Done(Tokens),
    /// A second factor is required; submit it with [`submit_mfa`].
    NeedsMfa { flow_id: String },
}

#[derive(Debug)]
pub enum LoginError {
    /// Couldn't reach or understand the server.
    Unreachable(String),
    InvalidCredentials,
    InvalidCode,
    Other(String),
}

impl LoginError {
    pub fn describe(&self) -> String {
        match self {
            Self::Unreachable(e) => format!("Couldn't reach Home Assistant ({e})."),
            Self::InvalidCredentials => "Wrong username or password.".into(),
            Self::InvalidCode => "That code is not valid.".into(),
            Self::Other(e) => e.clone(),
        }
    }
}

/// Refreshing failed because Home Assistant rejected the token (revoked or
/// the user was removed), as opposed to a network problem.
#[derive(Debug)]
pub struct TokenRejected;

impl std::fmt::Display for TokenRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Home Assistant rejected the stored login")
    }
}
impl std::error::Error for TokenRejected {}

pub fn client_id(screen_id: &str) -> String {
    format!("http://esp32-screen-{}.local/", screen_id.to_lowercase())
}

fn redirect_uri(client_id: &str) -> String {
    format!("{client_id}auth_callback")
}

/// Starts a login flow and submits the username and password.
pub fn login(
    base: &str,
    client_id: &str,
    username: &str,
    password: &str,
) -> Result<LoginStep, LoginError> {
    let (status, body) = post_json(
        &format!("{base}/auth/login_flow"),
        &json!({
            "client_id": client_id,
            "handler": ["homeassistant", null],
            "redirect_uri": redirect_uri(client_id),
        }),
    )
    .map_err(|e| LoginError::Unreachable(e.to_string()))?;
    if status != 200 {
        return Err(LoginError::Unreachable(format!(
            "HTTP {status}, is this Home Assistant?"
        )));
    }
    let flow_id = body["flow_id"]
        .as_str()
        .ok_or_else(|| LoginError::Unreachable("unexpected reply".into()))?
        .to_string();

    let (status, body) = post_json(
        &format!("{base}/auth/login_flow/{flow_id}"),
        &json!({ "client_id": client_id, "username": username, "password": password }),
    )
    .map_err(|e| LoginError::Unreachable(e.to_string()))?;
    flow_result(base, client_id, &flow_id, status, &body, false)
}

/// Submits a multi-factor authentication code for a flow that asked for one.
pub fn submit_mfa(
    base: &str,
    client_id: &str,
    flow_id: &str,
    code: &str,
) -> Result<LoginStep, LoginError> {
    let (status, body) = post_json(
        &format!("{base}/auth/login_flow/{flow_id}"),
        &json!({ "client_id": client_id, "code": code }),
    )
    .map_err(|e| LoginError::Unreachable(e.to_string()))?;
    flow_result(base, client_id, flow_id, status, &body, true)
}

fn flow_result(
    base: &str,
    client_id: &str,
    flow_id: &str,
    status: u16,
    body: &Value,
    mfa: bool,
) -> Result<LoginStep, LoginError> {
    if status != 200 {
        let message = body["message"].as_str().unwrap_or("").to_string();
        return Err(LoginError::Other(format!(
            "Login failed (HTTP {status}) {message}"
        )));
    }
    match body["type"].as_str() {
        Some("create_entry") => {
            let code = body["result"]
                .as_str()
                .ok_or_else(|| LoginError::Other("No authorization code in reply".into()))?;
            exchange_code(base, client_id, code).map(LoginStep::Done)
        }
        Some("form") => {
            let error = body["errors"]["base"].as_str().unwrap_or("");
            match (body["step_id"].as_str(), error) {
                (_, "invalid_auth") => Err(LoginError::InvalidCredentials),
                (_, "invalid_code") => Err(LoginError::InvalidCode),
                (Some("mfa"), _) if !mfa || error.is_empty() => Ok(LoginStep::NeedsMfa {
                    flow_id: flow_id.to_string(),
                }),
                (_, "") => Err(LoginError::Other(
                    "Home Assistant asked for an unsupported login step.".into(),
                )),
                (_, other) => Err(LoginError::Other(format!("Login failed: {other}"))),
            }
        }
        Some("abort") => {
            let reason = body["reason"].as_str().unwrap_or("aborted");
            Err(LoginError::Other(format!("Login aborted: {reason}")))
        }
        _ => Err(LoginError::Other(
            "Unexpected reply from Home Assistant.".into(),
        )),
    }
}

fn exchange_code(base: &str, client_id: &str, code: &str) -> Result<Tokens, LoginError> {
    let form = format!(
        "grant_type=authorization_code&code={}&client_id={}",
        url_encode(code),
        url_encode(client_id)
    );
    let (status, body) = post_form(&format!("{base}/auth/token"), &form)
        .map_err(|e| LoginError::Unreachable(e.to_string()))?;
    let (Some(access), Some(refresh)) = (
        body["access_token"].as_str(),
        body["refresh_token"].as_str(),
    ) else {
        return Err(LoginError::Other(format!(
            "Token request failed (HTTP {status})."
        )));
    };
    Ok(Tokens {
        access: access.into(),
        refresh: refresh.into(),
    })
}

/// Gets a fresh access token. Returns a [`TokenRejected`] error when Home
/// Assistant no longer accepts the refresh token.
pub fn refresh(base: &str, client_id: &str, refresh_token: &str) -> Result<String> {
    let form = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        url_encode(refresh_token),
        url_encode(client_id)
    );
    let (status, body) = post_form(&format!("{base}/auth/token"), &form)?;
    match (status, body["access_token"].as_str()) {
        (200, Some(token)) => Ok(token.to_string()),
        // 400 invalid_grant (revoked) or 403 (user disabled).
        (400 | 401 | 403, _) => Err(TokenRejected.into()),
        _ => bail!("token refresh failed with HTTP {status}"),
    }
}

/// Tells Home Assistant to revoke the refresh token (log out). Best effort.
pub fn revoke(base: &str, refresh_token: &str) {
    let form = format!("token={}&action=revoke", url_encode(refresh_token));
    if let Err(e) = post_form(&format!("{base}/auth/revoke"), &form) {
        log::warn!("revoking token failed: {e}");
    }
}

fn post_json(url: &str, body: &Value) -> Result<(u16, Value)> {
    post(url, "application/json", body.to_string().as_bytes())
}

fn post_form(url: &str, body: &str) -> Result<(u16, Value)> {
    post(url, "application/x-www-form-urlencoded", body.as_bytes())
}

fn post(url: &str, content_type: &str, body: &[u8]) -> Result<(u16, Value)> {
    let mut conn = EspHttpConnection::new(&Configuration {
        timeout: Some(Duration::from_secs(10)),
        crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
        ..Default::default()
    })?;
    let len = body.len().to_string();
    conn.initiate_request(
        Method::Post,
        url,
        &[("Content-Type", content_type), ("Content-Length", &len)],
    )
    .with_context(|| format!("connecting to {url}"))?;
    let mut written = 0;
    while written < body.len() {
        written += conn.write(&body[written..])?;
    }
    conn.initiate_response()?;
    let status = conn.status();
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = conn.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() > 64 * 1024 {
            return Err(anyhow!("reply too large"));
        }
    }
    let value = serde_json::from_slice(&out).unwrap_or(Value::Null);
    Ok((status, value))
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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

/// Normalizes what the user typed into a base URL: adds `http://` when no
/// scheme is given and drops trailing slashes.
pub fn normalize_url(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

/// `http://host:8123` → `ws://host:8123/api/websocket`.
pub fn websocket_url(base: &str) -> String {
    let rest = base
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| base.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| format!("ws://{base}"));
    format!("{rest}/api/websocket")
}
