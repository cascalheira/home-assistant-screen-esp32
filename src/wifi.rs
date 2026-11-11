//! Wi-Fi station manager.
//!
//! A worker thread owns the Wi-Fi driver. The UI sends it [`Command`]s and
//! receives [`Event`]s over channels, so a slow scan or connect never blocks
//! rendering.
//!
//! Credentials are written to NVS only after a connection with them has
//! succeeded. A device with stored credentials keeps retrying them forever
//! with backoff; new credentials typed into the wizard get a few attempts and
//! then fail with a reason the UI can show.

use crate::logic::i18n::{tr, trf};
use std::net::Ipv4Addr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use esp_idf_svc::eventloop::{EspSubscription, EspSystemEventLoop, System};
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::netif::IpEvent;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys;
use esp_idf_svc::wifi::{
    AuthMethod, ClientConfiguration, Configuration, EspWifi, PmfConfiguration, WifiEvent,
};

pub use crate::logic::net::wifi_bars as bars;

const NVS_NAMESPACE: &str = "wifi";
const NVS_SSID: &str = "ssid";
const NVS_PASSWORD: &str = "pass";

/// Time allowed for one association + DHCP attempt.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(20);
/// Attempts made with new (unsaved) credentials before giving up.
const NEW_CREDENTIAL_ATTEMPTS: u32 = 3;
/// Backoff cap when retrying saved credentials.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const MAX_SCAN_RESULTS: usize = 30;

#[derive(Debug)]
pub enum Command {
    Scan,
    /// Try new credentials once (with a few attempts); save them on success.
    Connect {
        ssid: String,
        password: String,
    },
    /// Connect with the stored credentials and keep retrying.
    ConnectSaved,
    /// Stop connecting / disconnect.
    Cancel,
    /// Disconnect and erase the stored credentials.
    Forget,
}

#[derive(Clone, Debug)]
pub struct Network {
    pub ssid: String,
    pub rssi: i8,
    pub secure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailReason {
    WrongPassword,
    NotFound,
    UnsupportedSecurity,
    Timeout,
    Lost,
    Other(u16),
}

impl FailReason {
    pub fn describe(self) -> String {
        match self {
            Self::WrongPassword => tr("The password seems to be wrong.").into(),
            Self::NotFound => tr("The network is out of range or the name is wrong.").into(),
            Self::UnsupportedSecurity => {
                tr("The network uses a security mode this screen doesn't support.").into()
            }
            Self::Timeout => tr("The network didn't respond in time.").into(),
            Self::Lost => tr("The connection was lost.").into(),
            Self::Other(code) => trf("Wi-Fi error (reason {}).", &[&code]),
        }
    }

    fn from_disconnect(reason: u16) -> Self {
        match reason as u32 {
            sys::wifi_err_reason_t_WIFI_REASON_AUTH_FAIL
            | sys::wifi_err_reason_t_WIFI_REASON_4WAY_HANDSHAKE_TIMEOUT
            | sys::wifi_err_reason_t_WIFI_REASON_HANDSHAKE_TIMEOUT
            | sys::wifi_err_reason_t_WIFI_REASON_MIC_FAILURE => Self::WrongPassword,
            sys::wifi_err_reason_t_WIFI_REASON_NO_AP_FOUND => Self::NotFound,
            sys::wifi_err_reason_t_WIFI_REASON_NO_AP_FOUND_W_COMPATIBLE_SECURITY
            | sys::wifi_err_reason_t_WIFI_REASON_NO_AP_FOUND_IN_AUTHMODE_THRESHOLD => {
                Self::UnsupportedSecurity
            }
            sys::wifi_err_reason_t_WIFI_REASON_BEACON_TIMEOUT => Self::Lost,
            _ => Self::Other(reason),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Connection {
    pub ssid: String,
    pub ip: Ipv4Addr,
    pub rssi: i8,
}

#[derive(Clone, Debug)]
pub enum Event {
    ScanStarted,
    ScanDone(Vec<Network>),
    ScanFailed(String),
    Connecting {
        ssid: String,
        attempt: u32,
    },
    Connected(Connection),
    /// Saved credentials failed; another attempt follows after `delay`.
    Retrying {
        ssid: String,
        reason: FailReason,
        delay: Duration,
    },
    /// New credentials failed for good. Nothing was saved.
    Failed {
        ssid: String,
        reason: FailReason,
    },
    /// Stopped on request (cancel or forget).
    Idle,
    /// Signal strength update while connected.
    Rssi(i8),
}

pub struct WifiHandle {
    pub control: WifiControl,
    pub events: Receiver<Event>,
    pub saved_ssid: Option<String>,
    pub mac: String,
}

/// Cloneable sender for [`Command`]s.
#[derive(Clone)]
pub struct WifiControl(Sender<Msg>);

impl WifiControl {
    pub fn send(&self, command: Command) {
        log::debug!("wifi command: {command:?}");
        // The worker lives for the whole program; a send error means it
        // panicked, which already aborts the firmware.
        let _ = self.0.send(Msg::Command(command));
    }
}

/// Starts the Wi-Fi worker thread.
pub fn start(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs_partition: EspDefaultNvsPartition,
) -> Result<WifiHandle> {
    let nvs = EspNvs::new(nvs_partition.clone(), NVS_NAMESPACE, true)?;
    let saved_ssid = load_credentials(&nvs).map(|(ssid, _)| ssid);

    let mut wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs_partition))?;
    // Start in station mode with an empty config so that scans work at once.
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        auth_method: AuthMethod::None,
        ..Default::default()
    }))?;
    wifi.start()?;
    let mac = wifi
        .sta_netif()
        .get_mac()
        .map(|m| {
            m.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .unwrap_or_default();

    let (msg_tx, msg_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();

    let tx = msg_tx.clone();
    let wifi_sub = sysloop.subscribe::<WifiEvent, _>(move |event| {
        let msg = match event {
            WifiEvent::StaDisconnected(d) => Some(Msg::Disconnected { reason: d.reason() }),
            _ => None,
        };
        if let Some(msg) = msg {
            let _ = tx.send(msg);
        }
    })?;
    let tx = msg_tx.clone();
    let ip_sub = sysloop.subscribe::<IpEvent, _>(move |event| {
        if let IpEvent::DhcpIpAssigned(a) = event {
            let _ = tx.send(Msg::GotIp { ip: a.ip() });
        }
    })?;

    let worker = Worker {
        wifi,
        nvs,
        events: event_tx,
        state: State::Idle,
        ignore_disconnects_until: Instant::now(),
        _subscriptions: [wifi_sub, ip_sub],
    };
    thread::Builder::new()
        .name("wifi".into())
        .stack_size(12 * 1024)
        .spawn(move || worker.run(msg_rx))?;

    Ok(WifiHandle {
        control: WifiControl(msg_tx),
        events: event_rx,
        saved_ssid,
        mac,
    })
}

fn load_credentials(nvs: &EspNvs<NvsDefault>) -> Option<(String, String)> {
    let mut ssid_buf = [0u8; 33];
    let mut pass_buf = [0u8; 65];
    let ssid = nvs
        .get_str(NVS_SSID, &mut ssid_buf)
        .ok()
        .flatten()?
        .to_string();
    let pass = nvs
        .get_str(NVS_PASSWORD, &mut pass_buf)
        .ok()
        .flatten()
        .unwrap_or("")
        .to_string();
    (!ssid.is_empty()).then_some((ssid, pass))
}

enum Msg {
    Command(Command),
    Disconnected { reason: u16 },
    GotIp { ip: Ipv4Addr },
}

#[derive(Clone)]
struct Target {
    ssid: String,
    password: String,
    /// Saved credentials: retry forever. New credentials: give up after a
    /// few attempts and save only on success.
    saved: bool,
}

enum State {
    Idle,
    Connecting {
        target: Target,
        attempt: u32,
        deadline: Instant,
        wrong_password: u32,
    },
    Waiting {
        target: Target,
        attempt: u32,
        until: Instant,
        wrong_password: u32,
    },
    Connected {
        target: Target,
        rssi: i8,
        next_rssi_check: Instant,
    },
}

struct Worker {
    wifi: EspWifi<'static>,
    nvs: EspNvs<NvsDefault>,
    events: Sender<Event>,
    state: State,
    /// Disconnect events up to this instant come from our own disconnect()
    /// calls and are ignored. Depending on the driver state they carry
    /// different reason codes, so the reason alone can't identify them.
    ignore_disconnects_until: Instant,
    _subscriptions: [EspSubscription<'static, System>; 2],
}

impl Worker {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Command(Command::Scan) => self.scan(),
            Msg::Command(Command::Connect { ssid, password }) => {
                self.stop();
                self.begin_attempt(
                    Target {
                        ssid,
                        password,
                        saved: false,
                    },
                    1,
                    0,
                );
            }
            Msg::Command(Command::ConnectSaved) => match load_credentials(&self.nvs) {
                Some((ssid, password)) => {
                    self.stop();
                    self.begin_attempt(
                        Target {
                            ssid,
                            password,
                            saved: true,
                        },
                        1,
                        0,
                    );
                }
                None => log::warn!("ConnectSaved without saved credentials"),
            },
            Msg::Command(Command::Cancel) => {
                self.stop();
                self.emit(Event::Idle);
                self.resume_saved();
            }
            Msg::Command(Command::Forget) => {
                self.stop();
                let _ = self.nvs.remove(NVS_SSID);
                let _ = self.nvs.remove(NVS_PASSWORD);
                log::info!("Wi-Fi credentials erased");
                self.emit(Event::Idle);
            }
            Msg::Disconnected { reason } => self.on_disconnected(reason),
            Msg::GotIp { ip } => self.on_got_ip(ip),
        }
    }

    /// Time-based transitions: attempt timeouts, retry timers, RSSI polling.
    fn tick(&mut self) {
        let now = Instant::now();
        match &mut self.state {
            State::Connecting { deadline, .. } if now >= *deadline => {
                log::warn!("connect attempt timed out");
                self.disconnect();
                self.attempt_failed(FailReason::Timeout);
            }
            State::Waiting { until, .. } if now >= *until => {
                let State::Waiting {
                    target,
                    attempt,
                    wrong_password,
                    ..
                } = std::mem::replace(&mut self.state, State::Idle)
                else {
                    unreachable!()
                };
                self.begin_attempt(target, attempt + 1, wrong_password);
            }
            State::Connected {
                rssi,
                next_rssi_check,
                ..
            } if now >= *next_rssi_check => {
                *next_rssi_check = now + Duration::from_secs(5);
                if let Ok(new) = self.wifi.get_rssi() {
                    let new = new.clamp(i8::MIN as i32, 0) as i8;
                    if new.abs_diff(*rssi) >= 3 {
                        *rssi = new;
                        let _ = self.events.send(Event::Rssi(new));
                    }
                }
            }
            _ => {}
        }
    }

    fn scan(&mut self) {
        // The driver can't scan while it is associating. A background attempt
        // on the saved network is paused for the scan and restarted after it;
        // an attempt the user just asked for is left alone.
        let mut resume = None;
        if let State::Connecting { target, .. } = &self.state {
            if !target.saved {
                self.emit(Event::ScanFailed(tr("Busy connecting").into()));
                return;
            }
            resume = Some(target.clone());
            self.stop();
        }
        self.do_scan();
        if let Some(target) = resume {
            self.begin_attempt(target, 1, 0);
        }
    }

    fn do_scan(&mut self) {
        self.emit(Event::ScanStarted);
        match self.wifi.scan() {
            Ok(aps) => {
                let mut networks: Vec<Network> = Vec::new();
                for ap in aps {
                    if ap.ssid.is_empty() {
                        continue; // hidden network
                    }
                    let secure = !matches!(ap.auth_method, None | Some(AuthMethod::None));
                    match networks.iter_mut().find(|n| n.ssid == ap.ssid.as_str()) {
                        Some(n) if n.rssi < ap.signal_strength => n.rssi = ap.signal_strength,
                        Some(_) => {}
                        None => networks.push(Network {
                            ssid: ap.ssid.to_string(),
                            rssi: ap.signal_strength,
                            secure,
                        }),
                    }
                }
                networks.sort_by_key(|n| core::cmp::Reverse(n.rssi));
                networks.truncate(MAX_SCAN_RESULTS);
                log::info!("scan found {} networks", networks.len());
                self.emit(Event::ScanDone(networks));
            }
            Err(e) => {
                log::warn!("scan failed: {e}");
                self.emit(Event::ScanFailed(e.to_string()));
            }
        }
    }

    fn begin_attempt(&mut self, target: Target, attempt: u32, wrong_password: u32) {
        let config = ClientConfiguration {
            ssid: match target.ssid.as_str().try_into() {
                Ok(s) => s,
                Err(_) => return self.give_up(target, FailReason::NotFound),
            },
            password: match target.password.as_str().try_into() {
                Ok(p) => p,
                Err(_) => return self.give_up(target, FailReason::WrongPassword),
            },
            auth_method: if target.password.is_empty() {
                AuthMethod::None
            } else {
                AuthMethod::WPA2Personal
            },
            // Needed for WPA3; still connects to WPA2 networks.
            pmf_cfg: PmfConfiguration::Capable { required: false },
            ..Default::default()
        };

        log::info!("connecting to '{}' (attempt {attempt})", target.ssid);
        self.emit(Event::Connecting {
            ssid: target.ssid.clone(),
            attempt,
        });

        let mut result = self
            .wifi
            .set_configuration(&Configuration::Client(config))
            .and_then(|_| {
                if self.wifi.is_started()? {
                    Ok(())
                } else {
                    self.wifi.start()
                }
            })
            .and_then(|_| self.wifi.connect());
        if matches!(&result, Err(e) if e.code() == sys::ESP_ERR_WIFI_CONN) {
            // The driver is still finishing an earlier attempt. Stop it and
            // start over with the new configuration.
            self.disconnect();
            thread::sleep(Duration::from_millis(200));
            result = self.wifi.connect();
        }
        self.state = State::Connecting {
            target,
            attempt,
            deadline: Instant::now() + ATTEMPT_TIMEOUT,
            wrong_password,
        };
        if let Err(e) = result {
            log::warn!("connect call failed: {e}");
            self.attempt_failed(FailReason::Other(0));
        }
    }

    fn on_disconnected(&mut self, reason: u16) {
        if Instant::now() < self.ignore_disconnects_until {
            log::debug!("ignoring disconnect (reason {reason}) caused by our own disconnect()");
            return;
        }
        match &self.state {
            State::Connecting { .. } => {
                // Our own disconnect() (timeout or cancel) reports ASSOC_LEAVE.
                if reason as u32 == sys::wifi_err_reason_t_WIFI_REASON_ASSOC_LEAVE {
                    return;
                }
                log::warn!("connect attempt failed, reason {reason}");
                self.attempt_failed(FailReason::from_disconnect(reason));
            }
            State::Connected { target, .. } => {
                log::warn!("connection to '{}' lost, reason {reason}", target.ssid);
                let target = Target {
                    saved: true,
                    ..target.clone()
                };
                self.emit(Event::Retrying {
                    ssid: target.ssid.clone(),
                    reason: FailReason::Lost,
                    delay: Duration::from_secs(1),
                });
                self.state = State::Waiting {
                    target,
                    attempt: 0,
                    until: Instant::now() + Duration::from_secs(1),
                    wrong_password: 0,
                };
            }
            // Late events from a connection we already gave up on.
            State::Idle | State::Waiting { .. } => {}
        }
    }

    fn attempt_failed(&mut self, reason: FailReason) {
        let State::Connecting {
            target,
            attempt,
            wrong_password,
            ..
        } = std::mem::replace(&mut self.state, State::Idle)
        else {
            return;
        };
        let wrong_password = wrong_password + (reason == FailReason::WrongPassword) as u32;

        if !target.saved {
            // Two handshake failures is a reliable wrong-password signal; a
            // single one can be a transient glitch.
            let final_reason = if wrong_password >= 2 {
                Some(FailReason::WrongPassword)
            } else if reason == FailReason::UnsupportedSecurity
                || attempt >= NEW_CREDENTIAL_ATTEMPTS
            {
                Some(reason)
            } else {
                None
            };
            if let Some(reason) = final_reason {
                return self.give_up(target, reason);
            }
            self.state = State::Waiting {
                target,
                attempt,
                until: Instant::now() + Duration::from_secs(1),
                wrong_password,
            };
            return;
        }

        let delay = Duration::from_secs(2u64.saturating_pow(attempt.min(6))).min(MAX_BACKOFF);
        log::info!("retrying '{}' in {}s", target.ssid, delay.as_secs());
        self.emit(Event::Retrying {
            ssid: target.ssid.clone(),
            reason,
            delay,
        });
        self.state = State::Waiting {
            target,
            attempt,
            until: Instant::now() + delay,
            wrong_password,
        };
    }

    fn give_up(&mut self, target: Target, reason: FailReason) {
        log::warn!("giving up on '{}': {reason:?}", target.ssid);
        self.disconnect();
        self.state = State::Idle;
        let was_saved = target.saved;
        self.emit(Event::Failed {
            ssid: target.ssid,
            reason,
        });
        // Fall back to the stored network after trying a new one. (Stored
        // credentials only end up here if they are unusable; resuming them
        // would loop.)
        if !was_saved {
            self.resume_saved();
        }
    }

    /// Reconnects to the stored network in the background, if there is one.
    fn resume_saved(&mut self) {
        if let Some((ssid, password)) = load_credentials(&self.nvs) {
            log::info!("resuming saved network '{ssid}'");
            self.begin_attempt(
                Target {
                    ssid,
                    password,
                    saved: true,
                },
                1,
                0,
            );
        }
    }

    fn on_got_ip(&mut self, ip: Ipv4Addr) {
        // The driver may finish an attempt we already counted as failed and
        // are waiting to retry; that still counts as success.
        let target = match &self.state {
            State::Connecting { target, .. } | State::Waiting { target, .. } => target.clone(),
            State::Idle | State::Connected { .. } => return,
        };
        if !target.saved {
            let saved = self
                .nvs
                .set_str(NVS_SSID, &target.ssid)
                .and_then(|_| self.nvs.set_str(NVS_PASSWORD, &target.password));
            match saved {
                Ok(()) => log::info!("saved credentials for '{}'", target.ssid),
                Err(e) => log::error!("could not save Wi-Fi credentials: {e}"),
            }
        }
        let rssi = self
            .wifi
            .get_rssi()
            .map(|r| r.clamp(i8::MIN as i32, 0) as i8)
            .unwrap_or(0);
        log::info!(
            "connected to '{}' with IP {ip}, RSSI {rssi} dBm",
            target.ssid
        );
        self.emit(Event::Connected(Connection {
            ssid: target.ssid.clone(),
            ip,
            rssi,
        }));
        self.state = State::Connected {
            target: Target {
                saved: true,
                ..target
            },
            rssi,
            next_rssi_check: Instant::now() + Duration::from_secs(5),
        };
    }

    /// Drops any connection or attempt, leaving the driver started.
    fn stop(&mut self) {
        if !matches!(self.state, State::Idle) {
            self.disconnect();
        }
        self.state = State::Idle;
    }

    fn disconnect(&mut self) {
        self.ignore_disconnects_until = Instant::now() + Duration::from_millis(500);
        let _ = self.wifi.disconnect();
    }
}
