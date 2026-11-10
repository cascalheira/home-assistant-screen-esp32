//! Home Assistant connection.
//!
//! A worker thread logs in, keeps the websocket connection to Home Assistant
//! alive and talks the `esp32_screen` protocol (docs/protocol.md). The UI
//! sends it [`Command`]s and receives [`Event`]s, like the Wi-Fi worker.
//!
//! Stored in NVS (namespace "ha"): the base URL and the refresh token. The
//! password is never stored.

pub mod auth;
pub mod model;
mod ws;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use esp_idf_svc::mdns::{EspMdns, Interface, Protocol, QueryResult};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use serde_json::{json, Value};

use auth::{LoginError, LoginStep, TokenRejected};
use model::{Instance, Layout, TileState};
use ws::{WebSocket, WsEvent};

const NVS_NAMESPACE: &str = "ha";
const NVS_URL: &str = "url";
const NVS_REFRESH: &str = "refresh";
/// POSIX TZ rule of Home Assistant's timezone, applied before HA connects.
const NVS_TZ: &str = "tz";
/// Version whose rollback was already reported to Home Assistant.
const NVS_ROLLBACK_REPORTED: &str = "rb_reported";

const SCREEN_NAME: &str = "ESP32 Screen";
const SCREEN_MODEL: &str = "ESP32-8048S043";
/// Give up on a connection attempt that hasn't subscribed by then.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Retry interval while the integration is missing in Home Assistant.
const NOT_INSTALLED_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum Command {
    /// Look for Home Assistant instances with mDNS.
    Discover,
    Login {
        url: String,
        username: String,
        password: String,
    },
    SubmitMfa {
        code: String,
    },
    Logout,
    /// Run a tile action (docs/protocol.md, `esp32_screen/action`).
    Action {
        tile_id: String,
        action: &'static str,
        value: Option<Value>,
    },
    /// Wi-Fi just came up: retry the connection now instead of waiting.
    NetworkUp,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No Home Assistant set up on this screen.
    NotConfigured,
    Connecting,
    Connected,
    /// Temporarily unreachable; retrying.
    Offline(String),
    /// The stored login no longer works; the user must sign in again.
    NeedsLogin(String),
    /// Connected, but the `esp32_screen` integration isn't installed.
    NotInstalled,
}

#[derive(Debug)]
pub enum Event {
    Discovered(Vec<Instance>),
    LoginFailed(String),
    MfaRequired,
    LoggedIn,
    LoggedOut,
    Status(Status),
    ScreenName(String),
    Layout(Layout),
    States(Vec<(String, TileState)>),
    ActionFailed(String),
}

pub struct HaHandle {
    tx: Sender<Msg>,
    pub events: Receiver<Event>,
    /// Base URL of the configured instance, if any.
    pub url: Option<String>,
}

impl HaHandle {
    pub fn control(&self) -> HaControl {
        HaControl(self.tx.clone())
    }
}

/// Cloneable command sender.
#[derive(Clone)]
pub struct HaControl(Sender<Msg>);

impl HaControl {
    pub fn send(&self, command: Command) {
        let _ = self.0.send(Msg::Command(command));
    }
}

pub(crate) enum Msg {
    Command(Command),
    Ws(u32, WsEvent),
}

fn parse_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Starts the Home Assistant worker. `screen_id` is the MAC address as 12
/// upper-case hex digits.
pub fn start(
    nvs_partition: EspDefaultNvsPartition,
    screen_id: String,
    updates: Sender<crate::ota::Status>,
) -> Result<HaHandle> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let stored = load(&nvs);
    let mut tz = [0u8; 96];
    if let Some(rule) = nvs.get_str(NVS_TZ, &mut tz).ok().flatten() {
        crate::saver::set_timezone(rule);
    }
    let url = stored.as_ref().map(|s| s.url.clone());

    let (tx, rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = Worker {
        client_id: auth::client_id(&screen_id),
        screen_id,
        nvs,
        stored,
        access: None,
        msgs: tx.clone(),
        events: event_tx,
        ws: None,
        generation: 0,
        phase: Phase::Idle,
        next_id: 1,
        pending: HashMap::new(),
        subscription: None,
        auth_sent: false,
        updates,
        backoff: 0,
        mfa: None,
        mdns: None,
    };
    thread::Builder::new()
        .name("ha".into())
        .stack_size(16 * 1024)
        .spawn(move || worker.run(rx))?;
    Ok(HaHandle {
        tx,
        events: event_rx,
        url,
    })
}

struct Stored {
    url: String,
    refresh: String,
}

fn load(nvs: &EspNvs<NvsDefault>) -> Option<Stored> {
    let mut url = [0u8; 256];
    let mut refresh = [0u8; 512];
    let url = nvs.get_str(NVS_URL, &mut url).ok().flatten()?.to_string();
    let refresh = nvs
        .get_str(NVS_REFRESH, &mut refresh)
        .ok()
        .flatten()?
        .to_string();
    (!url.is_empty() && !refresh.is_empty()).then_some(Stored { url, refresh })
}

#[derive(Debug, PartialEq)]
enum Phase {
    /// Not connecting (not configured, or waiting for a new login).
    Idle,
    /// Waiting to (re)connect.
    Retry { at: Instant },
    /// Websocket open, authenticating / registering / subscribing.
    Handshake { deadline: Instant },
    /// Subscribed; tile states are flowing.
    Live,
}

enum Pending {
    Register,
    Subscribe,
    Action,
}

struct Worker {
    screen_id: String,
    client_id: String,
    nvs: EspNvs<NvsDefault>,
    stored: Option<Stored>,
    access: Option<String>,
    msgs: Sender<Msg>,
    events: Sender<Event>,
    ws: Option<WebSocket>,
    generation: u32,
    phase: Phase,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    subscription: Option<u64>,
    /// The auth message went out on the current connection.
    auth_sent: bool,
    /// Firmware update progress, for the screen.
    updates: Sender<crate::ota::Status>,
    backoff: u32,
    /// Base URL and flow id of a login waiting for its second factor.
    mfa: Option<(String, String)>,
    mdns: Option<EspMdns>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Msg>) {
        if self.stored.is_some() {
            self.phase = Phase::Retry { at: Instant::now() };
        } else {
            self.emit(Event::Status(Status::NotConfigured));
        }
        loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Msg::Command(c)) => self.command(c),
                Ok(Msg::Ws(generation, event)) if generation == self.generation => {
                    self.ws_event(event)
                }
                Ok(Msg::Ws(..)) => {} // from a connection we already dropped
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn tick(&mut self) {
        match self.phase {
            Phase::Retry { at } if Instant::now() >= at => self.connect(),
            Phase::Handshake { deadline } if Instant::now() >= deadline => {
                self.fail("Home Assistant didn't answer in time.".into());
            }
            _ => {}
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Discover => {
                let found = self.discover();
                self.emit(Event::Discovered(found));
            }
            Command::Login {
                url,
                username,
                password,
            } => {
                let url = auth::normalize_url(&url);
                let result = auth::login(&url, &self.client_id, &username, &password);
                self.login_result(url, result);
            }
            Command::SubmitMfa { code } => match self.mfa.take() {
                Some((url, flow_id)) => {
                    let result = auth::submit_mfa(&url, &self.client_id, &flow_id, &code);
                    self.login_result(url, result);
                }
                None => self.emit(Event::LoginFailed("The login expired. Start again.".into())),
            },
            Command::Logout => {
                if let Some(stored) = self.stored.take() {
                    auth::revoke(&stored.url, &stored.refresh);
                }
                let _ = self.nvs.remove(NVS_URL);
                let _ = self.nvs.remove(NVS_REFRESH);
                self.disconnect();
                self.access = None;
                self.phase = Phase::Idle;
                self.emit(Event::Status(Status::NotConfigured));
                self.emit(Event::LoggedOut);
            }
            Command::Action {
                tile_id,
                action,
                value,
            } => {
                if self.phase != Phase::Live {
                    self.emit(Event::ActionFailed("Home Assistant is offline.".into()));
                    return;
                }
                let mut msg = json!({
                    "type": "esp32_screen/action",
                    "screen_id": self.screen_id,
                    "tile_id": tile_id,
                    "action": action,
                });
                if let Some(value) = value {
                    msg["value"] = value;
                }
                self.request(msg, Pending::Action);
            }
            Command::NetworkUp => {
                if matches!(self.phase, Phase::Retry { .. }) {
                    self.backoff = 0;
                    self.phase = Phase::Retry { at: Instant::now() };
                }
            }
        }
    }

    fn login_result(&mut self, url: String, result: Result<LoginStep, LoginError>) {
        match result {
            Ok(LoginStep::Done(tokens)) => {
                let saved = self
                    .nvs
                    .set_str(NVS_URL, &url)
                    .and_then(|_| self.nvs.set_str(NVS_REFRESH, &tokens.refresh));
                if let Err(e) = saved {
                    log::error!("could not store the Home Assistant login: {e}");
                }
                log::info!("logged in to Home Assistant at {url}");
                self.stored = Some(Stored {
                    url,
                    refresh: tokens.refresh,
                });
                self.access = Some(tokens.access);
                self.backoff = 0;
                self.emit(Event::LoggedIn);
                self.disconnect();
                self.connect();
            }
            Ok(LoginStep::NeedsMfa { flow_id }) => {
                self.mfa = Some((url, flow_id));
                self.emit(Event::MfaRequired);
            }
            Err(e) => {
                log::warn!("login failed: {e:?}");
                self.emit(Event::LoginFailed(e.describe()));
            }
        }
    }

    fn discover(&mut self) -> Vec<Instance> {
        if self.mdns.is_none() {
            match EspMdns::take() {
                Ok(m) => self.mdns = Some(m),
                Err(e) => {
                    log::warn!("mDNS unavailable: {e}");
                    return Vec::new();
                }
            }
        }
        let mdns = self.mdns.as_ref().unwrap();
        let empty = QueryResult {
            instance_name: None,
            hostname: None,
            port: 0,
            txt: Vec::new(),
            addr: Vec::new(),
            interface: Interface::STA,
            ip_protocol: Protocol::V4,
        };
        let mut results = vec![empty; 8];
        let n = match mdns.query_ptr(
            "_home-assistant",
            "_tcp",
            Duration::from_secs(3),
            8,
            &mut results,
        ) {
            Ok(n) => n,
            Err(e) => {
                log::info!("mDNS query found nothing ({e})");
                0
            }
        };
        let mut found: Vec<Instance> = Vec::new();
        for r in results.into_iter().take(n) {
            let txt = |key: &str| r.txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
            let url = txt("internal_url")
                .filter(|u| !u.is_empty())
                .or_else(|| txt("base_url").filter(|u| !u.is_empty()))
                .or_else(|| {
                    r.addr
                        .iter()
                        .find(|a| a.is_ipv4())
                        .map(|a| format!("http://{a}:{}", r.port))
                });
            let Some(url) = url else { continue };
            let url = auth::normalize_url(&url);
            let name = txt("location_name")
                .or(r.instance_name.clone())
                .unwrap_or_else(|| "Home Assistant".into());
            if !found.iter().any(|f| f.url == url) {
                found.push(Instance { name, url });
            }
        }
        log::info!("mDNS: {} Home Assistant instance(s)", found.len());
        found
    }

    /// Opens the websocket, refreshing the access token first if needed.
    fn connect(&mut self) {
        let Some(stored) = &self.stored else {
            self.phase = Phase::Idle;
            return;
        };
        let url = stored.url.clone();
        self.emit(Event::Status(Status::Connecting));

        if self.access.is_none() {
            match auth::refresh(&url, &self.client_id, &stored.refresh) {
                Ok(token) => self.access = Some(token),
                Err(e) if e.is::<TokenRejected>() => {
                    log::warn!("refresh token rejected");
                    self.phase = Phase::Idle;
                    self.emit(Event::Status(Status::NeedsLogin(
                        "Home Assistant no longer accepts this screen's login. Sign in again."
                            .into(),
                    )));
                    return;
                }
                Err(e) => {
                    log::warn!("token refresh failed: {e:#}");
                    return self.fail(format!("Can't reach Home Assistant ({e})."));
                }
            }
        }

        self.disconnect();
        self.generation = self.generation.wrapping_add(1);
        let uri = auth::websocket_url(&url);
        log::info!("connecting to {uri}");
        match WebSocket::connect(&uri, self.msgs.clone(), self.generation) {
            Ok(ws) => {
                self.ws = Some(ws);
                self.phase = Phase::Handshake {
                    deadline: Instant::now() + CONNECT_TIMEOUT,
                };
            }
            Err(e) => self.fail(format!("Can't open a connection ({e}).")),
        }
    }

    fn disconnect(&mut self) {
        self.ws = None;
        self.pending.clear();
        self.subscription = None;
        self.auth_sent = false;
    }

    /// Sends the auth message once per connection.
    ///
    /// It goes out as soon as the socket connects instead of waiting for
    /// `auth_required`: Home Assistant sends that greeting in the same TCP
    /// segment as the handshake reply, and ESP-IDF's websocket transport
    /// only hands such leftover bytes over after its read timeout, by which
    /// time Home Assistant's 10 s auth timeout has closed the connection.
    fn authenticate(&mut self) {
        if self.auth_sent {
            return;
        }
        self.auth_sent = true;
        let token = self.access.clone().unwrap_or_default();
        self.send(&json!({ "type": "auth", "access_token": token }));
    }

    /// Drops the connection and schedules a retry with backoff.
    fn fail(&mut self, reason: String) {
        self.disconnect();
        let delay = Duration::from_secs(2u64.saturating_pow(self.backoff.min(6))).min(MAX_BACKOFF);
        self.backoff += 1;
        log::warn!(
            "Home Assistant offline: {reason} Retrying in {}s.",
            delay.as_secs()
        );
        self.phase = Phase::Retry {
            at: Instant::now() + delay,
        };
        self.emit(Event::Status(Status::Offline(reason)));
    }

    fn request(&mut self, mut msg: Value, kind: Pending) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        msg["id"] = id.into();
        self.pending.insert(id, kind);
        self.send(&msg);
        id
    }

    fn send(&mut self, msg: &Value) {
        let Some(ws) = &self.ws else { return };
        if let Err(e) = ws.send(&msg.to_string()) {
            self.fail(format!("Sending failed ({e})."));
        }
    }

    fn ws_event(&mut self, event: WsEvent) {
        match event {
            WsEvent::Connected => {
                log::info!("websocket connected");
                self.authenticate();
            }
            WsEvent::Disconnected => {
                if self.ws.is_some() {
                    self.fail("Connection lost.".into());
                }
            }
            WsEvent::Text(text) => match serde_json::from_str::<Value>(&text) {
                Ok(msg) => self.message(msg),
                Err(e) => log::warn!("unparsable message from Home Assistant: {e}"),
            },
        }
    }

    fn message(&mut self, msg: Value) {
        match msg["type"].as_str().unwrap_or("") {
            "auth_required" => self.authenticate(),
            "auth_ok" => {
                log::info!(
                    "authenticated, Home Assistant {}",
                    msg["ha_version"].as_str().unwrap_or("?")
                );
                let register = json!({
                    "type": "esp32_screen/register",
                    "screen_id": self.screen_id,
                    "name": SCREEN_NAME,
                    "model": SCREEN_MODEL,
                    "sw_version": env!("CARGO_PKG_VERSION"),
                });
                self.request(register, Pending::Register);
            }
            "auth_invalid" => {
                // The access token expired or was revoked: get a new one.
                self.access = None;
                self.fail("Login expired, renewing.".into());
            }
            "result" => self.result(msg),
            "event" => {
                if msg["id"].as_u64().is_some() && msg["id"].as_u64() == self.subscription {
                    self.screen_event(&msg["event"]);
                }
            }
            "pong" => {}
            other => log::debug!("ignoring message type {other}"),
        }
    }

    fn result(&mut self, msg: Value) {
        let Some(id) = msg["id"].as_u64() else { return };
        let Some(kind) = self.pending.remove(&id) else {
            return;
        };
        let success = msg["success"].as_bool().unwrap_or(false);
        let code = msg["error"]["code"].as_str().unwrap_or("");
        let message = msg["error"]["message"].as_str().unwrap_or("").to_string();
        match kind {
            Pending::Register if success => {
                if let Some(name) = msg["result"]["name"].as_str() {
                    self.emit(Event::ScreenName(name.to_string()));
                }
                if let Some(rule) = msg["result"]["posix_tz"].as_str().filter(|r| !r.is_empty()) {
                    let mut buf = [0u8; 96];
                    let stored = self
                        .nvs
                        .get_str(NVS_TZ, &mut buf)
                        .ok()
                        .flatten()
                        .map(str::to_owned);
                    if stored.as_deref() != Some(rule) {
                        crate::saver::set_timezone(rule);
                        if let Err(e) = self.nvs.set_str(NVS_TZ, rule) {
                            log::warn!("could not store the timezone: {e}");
                        }
                    }
                }
                let sub = json!({ "type": "esp32_screen/subscribe", "screen_id": self.screen_id });
                let id = self.request(sub, Pending::Subscribe);
                self.subscription = Some(id);
            }
            Pending::Register if code == "unknown_command" => {
                log::warn!("esp32_screen integration not installed in Home Assistant");
                self.disconnect();
                self.phase = Phase::Retry {
                    at: Instant::now() + NOT_INSTALLED_RETRY,
                };
                self.emit(Event::Status(Status::NotInstalled));
            }
            Pending::Register | Pending::Subscribe if !success => {
                self.fail(format!("Home Assistant refused the screen ({message})."));
            }
            Pending::Subscribe => {
                log::info!("subscribed to screen {}", self.screen_id);
                self.backoff = 0;
                self.phase = Phase::Live;
                self.emit(Event::Status(Status::Connected));
                self.report_rollback();
            }
            Pending::Action if !success => {
                log::warn!("action failed: {code} {message}");
                let text = if message.is_empty() {
                    code.to_string()
                } else {
                    message
                };
                self.emit(Event::ActionFailed(text));
            }
            Pending::Register | Pending::Action => {}
        }
    }

    fn screen_event(&mut self, event: &Value) {
        match event["kind"].as_str() {
            Some("layout") => match serde_json::from_value::<Layout>(event["layout"].clone()) {
                Ok(layout) => {
                    log::info!("layout: {} page(s)", layout.pages.len());
                    self.emit(Event::Layout(layout));
                }
                Err(e) => log::warn!("bad layout: {e}"),
            },
            Some("states") => {
                let Some(tiles) = event["tiles"].as_object() else {
                    return;
                };
                let states = tiles
                    .iter()
                    .filter_map(|(id, v)| {
                        serde_json::from_value::<TileState>(v.clone())
                            .ok()
                            .map(|s| (id.clone(), s))
                    })
                    .collect();
                self.emit(Event::States(states));
            }
            Some("ota") => self.firmware_update(event),
            other => log::debug!("ignoring screen event {other:?}"),
        }
    }

    /// Downloads and installs firmware offered by Home Assistant, reporting
    /// progress with `esp32_screen/ota_status`. Restarts on success.
    fn firmware_update(&mut self, event: &Value) {
        use crate::ota::Status;
        let version = event["version"].as_str().unwrap_or("?").to_string();
        let size = event["size"].as_u64().unwrap_or(0) as usize;
        let path = event["url"].as_str().unwrap_or("").to_string();
        let sha256 = event["sha256"].as_str().and_then(parse_sha256);
        let Some(stored) = &self.stored else { return };
        let url = format!("{}{path}", stored.url);
        log::warn!(
            "firmware update {} -> {version} from Home Assistant",
            crate::ota::VERSION
        );
        let _ = self.updates.send(Status::Started {
            source: "Home Assistant",
            version,
        });

        let result = if size == 0 || path.is_empty() || sha256.is_none() {
            Err(anyhow::anyhow!("incomplete update offer"))
        } else {
            self.download_and_install(&url, size, sha256)
        };
        match result {
            Ok(()) => {
                self.report_ota("restarting", 100, None);
                let _ = self.updates.send(Status::Restarting);
                std::thread::sleep(Duration::from_millis(1500));
                unsafe { esp_idf_svc::sys::esp_restart() };
            }
            Err(e) => {
                let message = format!("{e:#}");
                log::error!("firmware update failed: {message}");
                self.report_ota("failed", 0, Some(&message));
                let _ = self.updates.send(Status::Failed(message));
            }
        }
    }

    fn download_and_install(
        &mut self,
        url: &str,
        size: usize,
        sha256: Option<[u8; 32]>,
    ) -> Result<()> {
        use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
        use esp_idf_svc::http::Method;
        let mut attempt = 0;
        let mut conn = loop {
            attempt += 1;
            let token = self.access.clone().unwrap_or_default();
            let mut conn = EspHttpConnection::new(&Configuration {
                timeout: Some(Duration::from_secs(30)),
                buffer_size: Some(4096),
                crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
                ..Default::default()
            })?;
            let auth = format!("Bearer {token}");
            conn.initiate_request(Method::Get, url, &[("Authorization", &auth)])?;
            conn.initiate_response()?;
            match conn.status() {
                200 => break conn,
                401 if attempt == 1 => {
                    // The access token expired: get a fresh one and retry.
                    let stored = self
                        .stored
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("signed out"))?;
                    self.access = Some(auth::refresh(
                        &stored.url,
                        &self.client_id,
                        &stored.refresh,
                    )?);
                }
                status => anyhow::bail!("download failed (HTTP {status})"),
            }
        };
        // Progress goes to the screen's UI and, every 5 %, to Home Assistant.
        // (Disjoint field borrows: `conn` is local, the closure only touches
        // the websocket, the id counter and the screen id.)
        let updates = self.updates.clone();
        let ws = &self.ws;
        let next_id = &mut self.next_id;
        let screen_id = &self.screen_id;
        let mut last_reported: Option<u8> = None;
        crate::ota::install(
            size,
            sha256,
            |buf| Ok(conn.read(buf)?),
            |status| {
                let report = match &status {
                    crate::ota::Status::Writing(pct)
                        if last_reported
                            .is_none_or(|last| *pct >= last.saturating_add(5) || *pct == 100) =>
                    {
                        last_reported = Some(*pct);
                        Some(("downloading", *pct))
                    }
                    crate::ota::Status::Verifying => Some(("verifying", 100)),
                    _ => None,
                };
                if let (Some((state, pct)), Some(ws)) = (report, ws) {
                    let id = *next_id;
                    *next_id += 1;
                    let msg = json!({
                        "id": id,
                        "type": "esp32_screen/ota_status",
                        "screen_id": screen_id,
                        "state": state,
                        "progress": pct,
                    });
                    let _ = ws.send(&msg.to_string());
                }
                let _ = updates.send(status);
            },
        )
    }

    /// Tells Home Assistant (once per bad version) that new firmware failed
    /// to start and the screen went back to the running version.
    fn report_rollback(&mut self) {
        let Some(bad) = crate::ota::rolled_back_version() else {
            return;
        };
        let mut buf = [0u8; 32];
        let reported = self
            .nvs
            .get_str(NVS_ROLLBACK_REPORTED, &mut buf)
            .ok()
            .flatten()
            .map(str::to_owned);
        if reported.as_deref() == Some(bad.as_str()) {
            return;
        }
        let message = format!(
            "firmware {bad} failed to start; rolled back to {}",
            crate::ota::VERSION
        );
        log::warn!("{message}");
        self.report_ota("failed", 0, Some(&message));
        let _ = self.nvs.set_str(NVS_ROLLBACK_REPORTED, &bad);
    }

    fn report_ota(&mut self, state: &str, progress: u8, error: Option<&str>) {
        let mut msg = json!({
            "type": "esp32_screen/ota_status",
            "screen_id": self.screen_id,
            "state": state,
            "progress": progress,
        });
        if let Some(error) = error {
            msg["error"] = error.into();
        }
        self.request(msg, Pending::Action);
    }
}
