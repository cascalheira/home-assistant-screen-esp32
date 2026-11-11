//! Home Assistant connection.
//!
//! A worker thread logs in, keeps the websocket connection to Home Assistant
//! alive and talks the `esp32_screen` protocol (docs/protocol.md). The UI
//! sends it [`Command`]s and receives [`Event`]s, like the Wi-Fi worker.
//!
//! Stored in NVS (namespace "ha"): the base URL and the refresh token. The
//! password is never stored.

pub mod auth;
mod cache;
pub mod model;
pub mod tls;
mod ws;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
/// Server whose certificate the user trusted on the screen (see tls.rs),
/// and that certificate's SHA-256.
const NVS_CERT_URL: &str = "cert_url";
const NVS_CERT_SHA: &str = "cert_sha";

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
    /// Answer to `Event::UntrustedCertificate`: trust it and sign in.
    TrustCertificate,
    /// Answer to `Event::UntrustedCertificate`: don't.
    RejectCertificate,
    Logout,
    /// Run a tile action (docs/protocol.md, `esp32_screen/action`).
    Action {
        tile_id: String,
        action: &'static str,
        value: Option<Value>,
    },
    /// Wi-Fi just came up: retry the connection now instead of waiting.
    NetworkUp,
    /// Report the backlight/screensaver state (`esp32_screen/state`).
    ReportState(crate::saver::ScreenState),
    /// The doorbell screen closed: stop the video.
    DoorbellClose,
    /// The doorbell screen's button (`esp32_screen/doorbell_action`).
    DoorbellAction(String),
    /// Debug console: show an MJPEG stream from any URL (no token sent).
    DoorbellTest(String),
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
    /// Signing in failed because the server's certificate isn't from a
    /// known authority. Carries its SHA-256 fingerprint (`AB:CD:…`); answer
    /// with `TrustCertificate` or `RejectCertificate`.
    UntrustedCertificate(String),
    LoggedIn,
    LoggedOut,
    Status(Status),
    ScreenName(String),
    Layout(Layout),
    States(Vec<(String, TileState)>),
    ActionFailed(String),
    /// Backlight/screensaver command from Home Assistant (section 6).
    Command(crate::saver::ScreenCommand),
    /// Clock settings (section 7).
    Settings {
        portuguese: bool,
        h24: bool,
    },
    /// Weather and indoor temperature for the clock (section 7).
    Info {
        condition: String,
        weather: String,
        indoor: String,
        /// `sun.sun`: whether the sun is up (None: no sun entity).
        sun: Option<bool>,
    },
    /// Daily forecast for the screensaver (section 7).
    Forecast(model::Forecast),
    /// Show a notification (section 8).
    Notify {
        id: String,
        title: String,
        message: String,
        image: Option<Picture>,
        timeout_secs: u64,
    },
    /// Close the notification with this id ("" = whatever is showing).
    Dismiss(String),
    /// The doorbell rang (section 9): show its camera.
    Doorbell {
        ring_id: String,
        title: String,
        timeout_secs: u64,
        /// Label of the button that opens the door/gate; "" = no button.
        action: String,
        /// A video stream follows (`DoorbellFrame`s).
        has_video: bool,
    },
    /// The next video frame. Decoded only once the previous one was shown
    /// (see `DOORBELL_FRAME_SHOWN`).
    DoorbellFrame(Picture),
    /// The video stopped before the doorbell screen closed.
    DoorbellVideoLost(String),
    /// The doorbell's button worked (true) or failed (false, with a reason).
    DoorbellActionDone(bool, String),
}

/// Set by the UI once it shows the last `DoorbellFrame`: the stream decodes
/// the next frame only then, so at most two frames are in memory and frames
/// the screen couldn't show anyway are skipped without decoding.
pub static DOORBELL_FRAME_SHOWN: AtomicBool = AtomicBool::new(true);

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
/// With `sd_card`, the last layout and states are cached on the card (see
/// cache.rs).
pub fn start(
    nvs_partition: EspDefaultNvsPartition,
    screen_id: String,
    updates: Sender<crate::ota::Status>,
    sd_card: bool,
) -> Result<HaHandle> {
    let nvs = EspNvs::new(nvs_partition, NVS_NAMESPACE, true)?;
    let stored = load(&nvs);
    let pin = load_pin(&nvs);
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
        legacy_state: false,
        stream: None,
        mfa: None,
        mdns: None,
        cache: sd_card.then(cache::Cache::default),
        pin,
        untrusted: None,
        trusted_now: None,
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

fn load_pin(nvs: &EspNvs<NvsDefault>) -> Option<(String, tls::Fingerprint)> {
    let mut url = [0u8; 256];
    let mut sha = [0u8; 32];
    let url = nvs
        .get_str(NVS_CERT_URL, &mut url)
        .ok()
        .flatten()?
        .to_string();
    let sha: tls::Fingerprint = nvs
        .get_blob(NVS_CERT_SHA, &mut sha)
        .ok()
        .flatten()?
        .try_into()
        .ok()?;
    Some((url, sha))
}

/// A sign-in waiting for the user to decide about the server's certificate.
struct UntrustedLogin {
    url: String,
    username: String,
    password: String,
    fingerprint: tls::Fingerprint,
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
    /// `esp32_screen/state`; failures are retried in the old format, never
    /// shown on the screen.
    State(crate::saver::ScreenState),
    DoorbellAction,
}

/// The doorbell video being received.
struct Stream {
    conn: StreamConnection,
    /// Bytes received and not yet part of a whole JPEG.
    buf: Vec<u8>,
    until: Instant,
    started: Instant,
    frames: u32,
    skipped: u32,
    /// The newest whole JPEG not decoded yet (older ones are dropped).
    pending: Option<Vec<u8>>,
    /// Two frame buffers, used in turn (see `decode_jpeg_into`).
    pool: [Option<Picture>; 2],
    next: usize,
    decode_time: Duration,
}

/// The video's HTTP connection. Created and used only on the worker's
/// thread; the worker (with `stream: None`) is moved there before any exists.
struct StreamConnection(esp_idf_svc::http::client::EspHttpConnection);
// SAFETY: see above; the connection never leaves the worker thread.
unsafe impl Send for StreamConnection {}

/// The doorbell video gives up on a frame larger than this.
const MAX_FRAME_BYTES: usize = 256 * 1024;
/// The stream's socket timeout: no data for this long ends the video.
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// The integration predates `screensaver_weather`/`screensaver_rotate`
    /// (before 0.4.0): report state without them. Reset on every subscribe.
    legacy_state: bool,
    /// Doorbell video, while the doorbell screen shows.
    stream: Option<Stream>,
    /// Base URL and flow id of a login waiting for its second factor.
    mfa: Option<(String, String)>,
    mdns: Option<EspMdns>,
    /// Layout and states on the SD card; None without a card.
    cache: Option<cache::Cache>,
    /// The certificate trusted for a server (saved in NVS).
    pin: Option<(String, tls::Fingerprint)>,
    /// Sign-in waiting for a decision about the server's certificate.
    untrusted: Option<UntrustedLogin>,
    /// Certificate trusted for a sign-in still in progress; saved once the
    /// sign-in succeeds.
    trusted_now: Option<(String, tls::Fingerprint)>,
}

impl Worker {
    /// Sends `esp32_screen/state` (docs/protocol.md section 6).
    fn report_state(&mut self, state: crate::saver::ScreenState) {
        let mut msg = json!({
            "type": "esp32_screen/state",
            "screen_id": self.screen_id,
            "brightness": state.brightness,
            "screensaver_mode": state.mode.name(),
            "screensaver_delay": state.delay_secs,
            "screensaver_active": state.active,
        });
        if !self.legacy_state {
            msg["screensaver_weather"] = json!(state.weather);
            msg["screensaver_rotate"] = json!(state.rotate_secs);
        }
        self.request(msg, Pending::State(state));
    }

    fn run(mut self, rx: Receiver<Msg>) {
        if let Some(stored) = &self.stored {
            // Show the last known dashboard while connecting.
            if self.cache.is_some() {
                let loaded = cache::load(&stored.url);
                if let Some((layout, states)) = &loaded {
                    log::info!("showing the cached layout until Home Assistant answers");
                    self.apply_layout(layout.clone());
                    self.apply_states(states);
                }
                self.cache = Some(cache::Cache::new(loaded.as_ref()));
            }
            self.phase = Phase::Retry { at: Instant::now() };
        } else {
            self.emit(Event::Status(Status::NotConfigured));
        }
        loop {
            // While the doorbell video runs, reading it paces the loop:
            // handle whatever arrived, then read the next frame.
            let wait = if self.stream.is_some() {
                Duration::ZERO
            } else {
                Duration::from_millis(500)
            };
            match rx.recv_timeout(wait) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while self.stream.is_some() {
                match rx.try_recv() {
                    Ok(msg) => self.handle(msg),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }
            if self.stream.is_some() {
                self.pump_stream();
            }
            self.tick();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Command(c) => self.command(c),
            Msg::Ws(generation, event) if generation == self.generation => self.ws_event(event),
            Msg::Ws(..) => {} // from a connection we already dropped
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn tick(&mut self) {
        if let (Some(cache), Some(stored)) = (&mut self.cache, &self.stored) {
            cache.tick(&stored.url);
        }
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
                self.trusted_now = None;
                self.trust_for(&url);
                let result = auth::login(&url, &self.client_id, &username, &password);
                if let Some(fingerprint) = self.untrusted_certificate(&url, &result) {
                    log::info!(
                        "{url} has an untrusted certificate {}",
                        tls::format(&fingerprint)
                    );
                    self.emit(Event::UntrustedCertificate(tls::format(&fingerprint)));
                    self.untrusted = Some(UntrustedLogin {
                        url,
                        username,
                        password,
                        fingerprint,
                    });
                    return;
                }
                self.login_result(url, result);
            }
            Command::TrustCertificate => {
                let Some(login) = self.untrusted.take() else {
                    return;
                };
                log::info!("certificate trusted for {}", login.url);
                self.trusted_now = Some((login.url.clone(), login.fingerprint));
                self.trust_for(&login.url);
                let result = auth::login(
                    &login.url,
                    &self.client_id,
                    &login.username,
                    &login.password,
                );
                self.login_result(login.url, result);
            }
            Command::RejectCertificate => self.untrusted = None,
            Command::SubmitMfa { code } => match self.mfa.take() {
                Some((url, flow_id)) => {
                    self.trust_for(&url);
                    let result = auth::submit_mfa(&url, &self.client_id, &flow_id, &code);
                    self.login_result(url, result);
                }
                None => self.emit(Event::LoginFailed("The login expired. Start again.".into())),
            },
            Command::Logout => {
                if let Some(stored) = self.stored.take() {
                    self.trust_for(&stored.url);
                    auth::revoke(&stored.url, &stored.refresh);
                }
                let _ = self.nvs.remove(NVS_URL);
                let _ = self.nvs.remove(NVS_REFRESH);
                let _ = self.nvs.remove(NVS_CERT_URL);
                let _ = self.nvs.remove(NVS_CERT_SHA);
                self.pin = None;
                self.trusted_now = None;
                tls::set_pin(None);
                if let Some(cache) = &mut self.cache {
                    cache.clear();
                }
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
            Command::ReportState(state) => {
                if self.phase == Phase::Live {
                    self.report_state(state);
                }
            }
            Command::DoorbellClose => self.stop_stream("closed"),
            Command::DoorbellAction(ring_id) => {
                let msg = json!({
                    "type": "esp32_screen/doorbell_action",
                    "screen_id": self.screen_id,
                    "ring_id": ring_id,
                });
                self.request(msg, Pending::DoorbellAction);
            }
            Command::DoorbellTest(url) => {
                self.emit(Event::Doorbell {
                    ring_id: String::new(),
                    title: "Test video".into(),
                    timeout_secs: 60,
                    action: String::new(),
                    has_video: true,
                });
                self.start_stream(&url, 60, false);
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
                // A certificate trusted for this sign-in now belongs to the
                // server; one trusted for another server is dropped.
                if let Some((pin_url, sha)) = self.trusted_now.take().filter(|(u, _)| *u == url) {
                    let saved = self
                        .nvs
                        .set_str(NVS_CERT_URL, &pin_url)
                        .and_then(|_| self.nvs.set_blob(NVS_CERT_SHA, &sha));
                    if let Err(e) = saved {
                        log::error!("could not store the trusted certificate: {e}");
                    }
                    self.pin = Some((pin_url, sha));
                } else if self.pin.as_ref().is_some_and(|(u, _)| *u != url) {
                    let _ = self.nvs.remove(NVS_CERT_URL);
                    let _ = self.nvs.remove(NVS_CERT_SHA);
                    self.pin = None;
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

    /// Selects the certificate check for a connection to `url`: the
    /// certificate trusted for it, or public authorities.
    fn trust_for(&self, url: &str) {
        let pin = [&self.trusted_now, &self.pin]
            .into_iter()
            .flatten()
            .find(|(u, _)| u == url)
            .map(|(_, sha)| *sha);
        tls::set_pin(pin);
    }

    /// After a failed sign-in: the certificate the server presented, if
    /// failing to trust it is why the connection failed.
    fn untrusted_certificate(
        &self,
        url: &str,
        result: &Result<LoginStep, LoginError>,
    ) -> Option<tls::Fingerprint> {
        if !url.starts_with("https://") || !matches!(result, Err(LoginError::Unreachable(_))) {
            return None;
        }
        let seen = tls::probe(|| auth::probe(url))?;
        // The trusted certificate itself: the failure has another cause.
        let trusted = self
            .pin
            .as_ref()
            .is_some_and(|(u, sha)| u == url && *sha == seen);
        (!trusted).then_some(seen)
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
        self.trust_for(&url);

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
        if tls::take_refused().is_some() {
            // The trusted certificate was replaced: someone has to look at
            // the new one before the screen talks to the server again.
            log::warn!("Home Assistant presented a different certificate");
            self.phase = Phase::Idle;
            self.emit(Event::Status(Status::NeedsLogin(
                "Home Assistant's certificate changed. Sign in again to check the new one.".into(),
            )));
            return;
        }
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
            Pending::State(state) if !success => {
                if code == "invalid_format" && !self.legacy_state {
                    log::info!("Home Assistant's integration predates the forecast screensaver");
                    self.legacy_state = true;
                    self.report_state(state);
                } else {
                    log::warn!("state report failed: {code} {message}");
                }
            }
            Pending::State(_) => {}
            Pending::DoorbellAction => {
                if !success {
                    log::warn!("doorbell action failed: {code} {message}");
                }
                let text = if message.is_empty() {
                    code.to_string()
                } else {
                    message
                };
                self.emit(Event::DoorbellActionDone(success, text));
            }
            Pending::Subscribe => {
                log::info!("subscribed to screen {}", self.screen_id);
                self.legacy_state = false;
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

    /// Sends a layout to the UI. False if it doesn't parse.
    fn apply_layout(&self, layout: Value) -> bool {
        match serde_json::from_value::<Layout>(layout) {
            Ok(layout) => {
                log::info!("layout: {} page(s)", layout.pages.len());
                self.emit(Event::Layout(layout));
                true
            }
            Err(e) => {
                log::warn!("bad layout: {e}");
                false
            }
        }
    }

    fn apply_states(&self, tiles: &serde_json::Map<String, Value>) {
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

    fn screen_event(&mut self, event: &Value) {
        match event["kind"].as_str() {
            Some("layout") => {
                if self.apply_layout(event["layout"].clone()) {
                    if let (Some(cache), Some(stored)) = (&mut self.cache, &self.stored) {
                        cache.set_layout(&stored.url, &event["layout"]);
                    }
                }
            }
            Some("states") => {
                let Some(tiles) = event["tiles"].as_object() else {
                    return;
                };
                self.apply_states(tiles);
                if let Some(cache) = &mut self.cache {
                    cache.merge_states(tiles);
                }
            }
            Some("ota") => self.firmware_update(event),
            Some("command") => {
                use crate::saver::{Mode, ScreenCommand};
                let value = &event["value"];
                let command = match event["command"].as_str().unwrap_or("") {
                    "brightness" => value
                        .as_u64()
                        .map(|v| ScreenCommand::Brightness(v.min(100) as u8)),
                    "screensaver_mode" => value
                        .as_str()
                        .and_then(Mode::parse)
                        .map(ScreenCommand::SaverMode),
                    "screensaver_delay" => {
                        value.as_u64().map(|v| ScreenCommand::SaverDelay(v as u32))
                    }
                    "screensaver_weather" => value.as_bool().map(ScreenCommand::SaverWeather),
                    "screensaver_rotate" => {
                        value.as_u64().map(|v| ScreenCommand::SaverRotate(v as u32))
                    }
                    "wake" => Some(ScreenCommand::Wake),
                    "sleep" => Some(ScreenCommand::Sleep),
                    _ => None,
                };
                match command {
                    Some(command) => self.emit(Event::Command(command)),
                    None => log::warn!("ignoring command {event}"),
                }
            }
            Some("settings") => {
                let settings = &event["settings"];
                self.emit(Event::Settings {
                    portuguese: settings["language"].as_str() == Some("pt"),
                    h24: settings["clock_24h"].as_bool().unwrap_or(true),
                });
            }
            Some("notify") => {
                let text = |key: &str| event[key].as_str().unwrap_or("").to_string();
                let url = text("image_url");
                log::info!(
                    "notification {} received (image: {})",
                    text("id"),
                    !url.is_empty()
                );
                let image = if url.is_empty() {
                    None
                } else {
                    let base = self
                        .stored
                        .as_ref()
                        .map(|s| s.url.clone())
                        .unwrap_or_default();
                    match self.fetch_image(&format!("{base}{url}")) {
                        Ok(image) => Some(image),
                        Err(e) => {
                            log::warn!("notification image: {e:#}");
                            None
                        }
                    }
                };
                self.emit(Event::Notify {
                    id: text("id"),
                    title: text("title"),
                    message: text("message"),
                    image,
                    timeout_secs: event["timeout"].as_u64().unwrap_or(30),
                });
            }
            Some("dismiss") => {
                self.emit(Event::Dismiss(
                    event["id"].as_str().unwrap_or("").to_string(),
                ));
            }
            Some("forecast") => match serde_json::from_value::<model::Forecast>(event.clone()) {
                Ok(forecast) => self.emit(Event::Forecast(forecast)),
                Err(e) => log::warn!("ignoring a forecast: {e}"),
            },
            Some("doorbell") => {
                let text = |key: &str| event[key].as_str().unwrap_or("").to_string();
                let url = text("stream_url");
                let timeout = event["timeout"].as_u64().unwrap_or(60).clamp(5, 600);
                log::info!("doorbell {} (video: {})", text("id"), !url.is_empty());
                self.stop_stream("rang again");
                self.emit(Event::Doorbell {
                    ring_id: text("id"),
                    title: text("title"),
                    timeout_secs: timeout,
                    action: text("action"),
                    has_video: !url.is_empty(),
                });
                if !url.is_empty() {
                    let base = self
                        .stored
                        .as_ref()
                        .map(|s| s.url.clone())
                        .unwrap_or_default();
                    self.start_stream(&format!("{base}{url}"), timeout, true);
                }
            }
            Some("info") => {
                let text = |key: &str| event[key].as_str().unwrap_or("").to_string();
                self.emit(Event::Info {
                    condition: text("weather_condition"),
                    weather: text("weather_temperature"),
                    indoor: text("indoor_temperature"),
                    sun: match event["sun"].as_str() {
                        Some("above_horizon") => Some(true),
                        Some("below_horizon") => Some(false),
                        _ => None,
                    },
                });
            }
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

    /// GET from Home Assistant with the screen's access token, refreshing it
    /// once if it expired. Returns the open connection with status 200.
    fn authenticated_get(
        &mut self,
        url: &str,
    ) -> Result<esp_idf_svc::http::client::EspHttpConnection> {
        self.authenticated_get_with(url, Duration::from_secs(30))
    }

    /// `authenticated_get` with a socket timeout.
    fn authenticated_get_with(
        &mut self,
        url: &str,
        timeout: Duration,
    ) -> Result<esp_idf_svc::http::client::EspHttpConnection> {
        use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
        use esp_idf_svc::http::Method;
        if let Some(stored) = &self.stored {
            self.trust_for(&stored.url);
        }
        let mut attempt = 0;
        let conn = loop {
            attempt += 1;
            let token = self.access.clone().unwrap_or_default();
            let mut conn = EspHttpConnection::new(&Configuration {
                timeout: Some(timeout),
                buffer_size: Some(4096),
                crt_bundle_attach: Some(tls::attach),
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
                status => anyhow::bail!("HTTP {status}"),
            }
        };
        Ok(conn)
    }

    /// Opens the doorbell video (`multipart/x-mixed-replace` of JPEGs).
    fn start_stream(&mut self, url: &str, timeout_secs: u64, with_token: bool) {
        use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
        use esp_idf_svc::http::Method;
        let opened = if with_token {
            self.authenticated_get_with(url, STREAM_READ_TIMEOUT)
        } else {
            EspHttpConnection::new(&Configuration {
                timeout: Some(STREAM_READ_TIMEOUT),
                buffer_size: Some(4096),
                ..Default::default()
            })
            .map_err(anyhow::Error::from)
            .and_then(|mut conn| {
                conn.initiate_request(Method::Get, url, &[])?;
                conn.initiate_response()?;
                match conn.status() {
                    200 => Ok(conn),
                    status => anyhow::bail!("HTTP {status}"),
                }
            })
        };
        match opened {
            Ok(conn) => {
                log::info!("doorbell video opened");
                DOORBELL_FRAME_SHOWN.store(true, Ordering::SeqCst);
                let now = Instant::now();
                self.stream = Some(Stream {
                    conn: StreamConnection(conn),
                    buf: Vec::with_capacity(64 * 1024),
                    until: now + Duration::from_secs(timeout_secs + 5),
                    started: now,
                    frames: 0,
                    skipped: 0,
                    pending: None,
                    pool: [None, None],
                    next: 0,
                    decode_time: Duration::ZERO,
                });
            }
            Err(e) => {
                log::warn!("doorbell video: {e:#}");
                self.emit(Event::DoorbellVideoLost(format!("{e:#}")));
            }
        }
    }

    fn stop_stream(&mut self, why: &str) {
        if let Some(stream) = self.stream.take() {
            let secs = stream.started.elapsed().as_secs_f32().max(0.1);
            log::info!(
                "doorbell video {why}: {} frames shown, {} skipped, {:.1} fps, decoding {} ms a frame",
                stream.frames,
                stream.skipped,
                stream.frames as f32 / secs,
                stream.decode_time.as_millis() / stream.frames.max(1) as u128
            );
        }
    }

    /// Decodes the newest frame as soon as the UI has shown the previous
    /// one; otherwise reads more of the video (one chunk, so the UI's
    /// readiness is checked often), keeping only the newest whole JPEG.
    fn pump_stream(&mut self) {
        let Some(stream) = &mut self.stream else {
            return;
        };
        if Instant::now() >= stream.until {
            self.stop_stream("timed out");
            return;
        }
        if stream.pending.is_some() && DOORBELL_FRAME_SHOWN.swap(false, Ordering::SeqCst) {
            let frame = stream.pending.take().unwrap_or_default();
            let t = Instant::now();
            let slot = stream.next;
            match decode_jpeg_into(frame, false, stream.pool[slot].take()) {
                Ok(picture) => {
                    stream.decode_time += t.elapsed();
                    stream.frames += 1;
                    stream.next = 1 - slot;
                    stream.pool[slot] = Some(picture.clone());
                    self.emit(Event::DoorbellFrame(picture));
                }
                Err(e) => {
                    DOORBELL_FRAME_SHOWN.store(true, Ordering::SeqCst);
                    stream.skipped += 1;
                    log::warn!("doorbell frame: {e:#}");
                }
            }
            return;
        }
        let mut chunk = [0u8; 4096];
        match stream.conn.0.read(&mut chunk) {
            Ok(0) => {
                self.stop_stream("ended");
                self.emit(Event::DoorbellVideoLost("the video ended".into()));
            }
            Ok(n) => {
                stream.buf.extend_from_slice(&chunk[..n]);
                while let Some(frame) = crate::logic::image::take_jpeg(&mut stream.buf) {
                    if stream.pending.replace(frame).is_some() {
                        stream.skipped += 1;
                    }
                }
                if stream.buf.len() > MAX_FRAME_BYTES {
                    log::warn!("doorbell video: frame too large, resyncing");
                    stream.buf.clear();
                }
            }
            Err(e) => {
                log::warn!("doorbell video: {e}");
                self.stop_stream("failed");
                self.emit(Event::DoorbellVideoLost(e.to_string()));
            }
        }
    }

    /// Downloads a notification image (at most 256 KB) and decodes the JPEG.
    fn fetch_image(&mut self, url: &str) -> Result<Picture> {
        use anyhow::Context;
        let mut conn = self.authenticated_get(url).context("request")?;
        let mut data = Vec::new();
        let mut buf = [0u8; 2048];
        loop {
            let n = conn.read(&mut buf).context("download")?;
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            if data.len() > 256 * 1024 {
                anyhow::bail!("image too large");
            }
        }
        log::info!("notification image: {} bytes", data.len());
        decode_jpeg(data, true)
    }

    fn download_and_install(
        &mut self,
        url: &str,
        size: usize,
        sha256: Option<[u8; 32]>,
    ) -> Result<()> {
        let mut conn = self.authenticated_get(url)?;
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

/// A decoded notification picture, ready for Slint.
pub type Picture = slint::SharedPixelBuffer<slint::Rgb8Pixel>;

/// Pictures larger than this are refused: PSRAM has room for one picture
/// of about this size next to the firmware and the frame buffers.
const MAX_PICTURE_PIXELS: usize = 640 * 400;
/// PSRAM that must stay free after a picture is decoded.
const PSRAM_RESERVE: usize = 96 * 1024;

/// Decodes a baseline JPEG with Espressif's esp_jpeg component (TJpgDec from
/// the chip's ROM: a few KB of working memory, little stack). The pixels go
/// straight into the buffer the UI will show.
fn decode_jpeg(data: Vec<u8>, verbose: bool) -> Result<Picture> {
    decode_jpeg_into(data, verbose, None)
}

/// `decode_jpeg`, reusing `reuse` when it has the right size and nothing
/// else holds it (no allocation: the doorbell video alternates between two
/// buffers, which keeps PSRAM from fragmenting).
fn decode_jpeg_into(mut data: Vec<u8>, verbose: bool, reuse: Option<Picture>) -> Result<Picture> {
    use esp_idf_svc::sys::{esp, jpeg};
    let mut cfg = jpeg::esp_jpeg_image_cfg_t {
        indata: data.as_mut_ptr(),
        indata_size: data.len() as u32,
        out_format: jpeg::esp_jpeg_image_format_t_JPEG_IMAGE_FORMAT_RGB888,
        out_scale: jpeg::esp_jpeg_image_scale_t_JPEG_IMAGE_SCALE_0,
        ..Default::default()
    };
    let mut info = jpeg::esp_jpeg_image_output_t::default();
    esp!(unsafe { jpeg::esp_jpeg_get_image_info(&mut cfg, &mut info) })
        .map_err(|e| anyhow::anyhow!("not a JPEG the screen can read ({e})"))?;
    let (width, height) = (info.width as usize, info.height as usize);
    if width * height > MAX_PICTURE_PIXELS {
        anyhow::bail!("image too large ({width}x{height})");
    }
    let reuse = reuse.filter(|p| p.width() as usize == width && p.height() as usize == height);
    let mut picture = match reuse {
        Some(picture) => picture,
        None => {
            // Leave PSRAM for everything else: the firmware runs from PSRAM,
            // so only a few hundred KB are free to begin with.
            let needed = width * height * 3;
            let largest = unsafe {
                esp_idf_svc::sys::heap_caps_get_largest_free_block(
                    esp_idf_svc::sys::MALLOC_CAP_SPIRAM,
                )
            };
            if largest < needed + PSRAM_RESERVE {
                anyhow::bail!(
                    "not enough memory for a {width}x{height} picture ({largest} B free)"
                );
            }
            Picture::new(width as u32, height as u32)
        }
    };
    let out = picture.make_mut_bytes();
    cfg.outbuf = out.as_mut_ptr();
    cfg.outbuf_size = out.len() as u32;
    esp!(unsafe { jpeg::esp_jpeg_decode(&mut cfg, &mut info) })
        .map_err(|e| anyhow::anyhow!("decoding failed ({e})"))?;
    if verbose {
        log::info!("notification image {width}x{height}");
    }
    Ok(picture)
}
