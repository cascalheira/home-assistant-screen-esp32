//! Glue between the Slint UI and the two workers (Wi-Fi and Home Assistant).
//!
//! Owns the wizard's state machine, handles UI callbacks, and turns worker
//! events into page changes and dashboard updates. Everything here runs on
//! the UI thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::json;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::ha::model::{Layout, Tile, TileState};
use crate::ha::{self, HaControl, HaHandle, Status};
use crate::wifi::{self, Command, Connection, FailReason, WifiControl, WifiHandle};
use crate::{AppWindow, InstanceItem, ModeItem, NetworkItem, Page, PageTab, TileData};

const MAX_SSID_BYTES: usize = 32;
const MAX_WIFI_PASSWORD: usize = 63;
const MIN_WIFI_PASSWORD: usize = 8;
/// Wait this long after the last +/- tap before sending a new temperature.
const TEMPERATURE_DEBOUNCE: Duration = Duration::from_millis(700);
/// Keep showing a sent temperature this long while HA catches up.
const TEMPERATURE_HOLD: Duration = Duration::from_secs(4);

/// What the text entry page is collecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    WifiSsid,
    WifiPassword,
    HaUrl,
    HaUser,
    HaPassword,
    HaMfa,
}

impl Entry {
    fn max_chars(self) -> usize {
        match self {
            Self::WifiSsid => MAX_SSID_BYTES,
            Self::WifiPassword => MAX_WIFI_PASSWORD,
            Self::HaUrl => 200,
            Self::HaUser => 64,
            Self::HaPassword => 128,
            Self::HaMfa => 10,
        }
    }
}

/// What the progress page is waiting for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    /// Joining Wi-Fi. `boot`: reconnecting with saved credentials at start-up.
    Wifi {
        boot: bool,
    },
    HaLogin,
}

/// What the result page is reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    WifiOk,
    WifiFailed,
    HaOk,
    HaFailed,
}

/// What the button on the dashboard's message does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DashAction {
    None,
    SetUp,
    SignIn,
}

struct State {
    entry_mode: Entry,
    entry: String,
    revealed: bool,

    // Wi-Fi
    selected_ssid: String,
    /// False for hidden networks typed by name: an empty password is allowed.
    security_known: bool,
    selected_open: bool,
    last_password: String,
    connection: Option<Connection>,
    progress: Progress,
    outcome: Outcome,

    // Home Assistant
    ha_url: Option<String>,
    ha_url_input: String,
    ha_user: String,
    ha_status: Status,
    ha_find_return: Page,
    screen_name: String,
    layout: Layout,
    states: HashMap<String, TileState>,
    page_index: usize,
    dash_action: DashAction,

    // Pop-ups
    climate_tile: Option<String>,
    /// Temperature chosen with +/-: (value, send at, hold display until).
    /// `send at` is None once sent.
    pending_target: Option<(f64, Option<Instant>, Instant)>,
    dimmer_tile: Option<String>,
    dimmer_touched: Option<Instant>,
}

pub struct App {
    shared: Rc<Shared>,
    wifi_events: Receiver<wifi::Event>,
    ha_events: Receiver<ha::Event>,
    updates: Receiver<crate::ota::Status>,
}

struct Shared {
    ui: Weak<AppWindow>,
    wifi: WifiControl,
    ha: HaControl,
    networks: Rc<VecModel<NetworkItem>>,
    instances: Rc<VecModel<InstanceItem>>,
    pages: Rc<VecModel<PageTab>>,
    tiles: Rc<VecModel<TileData>>,
    modes: Rc<VecModel<ModeItem>>,
    st: RefCell<State>,
}

impl App {
    pub fn new(
        ui: &AppWindow,
        wifi: WifiHandle,
        ha: HaHandle,
        updates: Receiver<crate::ota::Status>,
    ) -> Self {
        let shared = Rc::new(Shared {
            ui: ui.as_weak(),
            wifi: wifi.control,
            ha: ha.control(),
            networks: Rc::new(VecModel::default()),
            instances: Rc::new(VecModel::default()),
            pages: Rc::new(VecModel::default()),
            tiles: Rc::new(VecModel::default()),
            modes: Rc::new(VecModel::default()),
            st: RefCell::new(State {
                entry_mode: Entry::WifiPassword,
                entry: String::new(),
                revealed: false,
                selected_ssid: String::new(),
                security_known: true,
                selected_open: false,
                last_password: String::new(),
                connection: None,
                progress: Progress::Wifi { boot: false },
                outcome: Outcome::WifiOk,
                ha_url: ha.url.clone(),
                ha_url_input: ha.url.clone().unwrap_or_default(),
                ha_user: String::new(),
                ha_status: if ha.url.is_some() {
                    Status::Connecting
                } else {
                    Status::NotConfigured
                },
                ha_find_return: Page::Networks,
                screen_name: "ESP32 Screen".into(),
                layout: Layout::default(),
                states: HashMap::new(),
                page_index: 0,
                dash_action: DashAction::None,
                climate_tile: None,
                pending_target: None,
                dimmer_tile: None,
                dimmer_touched: None,
            }),
        });

        ui.set_networks(ModelRc::from(shared.networks.clone()));
        ui.set_ha_instances(ModelRc::from(shared.instances.clone()));
        ui.set_dash_pages(ModelRc::from(shared.pages.clone()));
        ui.set_tiles(ModelRc::from(shared.tiles.clone()));
        ui.set_climate_modes(ModelRc::from(shared.modes.clone()));
        ui.set_mac_address(wifi.mac.clone().into());
        ui.set_screen_name("ESP32 Screen".into());
        ui.set_firmware_version(crate::ota::VERSION.into());
        shared.register_callbacks(ui);
        shared.refresh_settings();
        shared.refresh_dashboard();

        match wifi.saved_ssid {
            Some(ssid) => {
                shared.st.borrow_mut().progress = Progress::Wifi { boot: true };
                shared.show_progress(
                    &format!("Connecting to {ssid}…"),
                    "Using the saved network.",
                    "",
                );
                shared.wifi.send(Command::ConnectSaved);
            }
            None => ui.set_page(Page::Welcome),
        }

        Self {
            shared,
            wifi_events: wifi.events,
            ha_events: ha.events,
            updates,
        }
    }

    /// Applies pending worker events and timers. Call from the UI loop.
    pub fn poll(&self) {
        while let Ok(event) = self.wifi_events.try_recv() {
            self.shared.on_wifi_event(event);
        }
        while let Ok(event) = self.ha_events.try_recv() {
            self.shared.on_ha_event(event);
        }
        while let Ok(status) = self.updates.try_recv() {
            self.shared.on_update(status);
        }
        self.shared.tick();
    }

    pub fn forget_wifi(&self) {
        self.shared.forget_wifi();
    }

    pub fn wifi_connected(&self) -> bool {
        self.shared.st.borrow().connection.is_some()
    }

    pub fn status(&self) -> String {
        let st = self.shared.st.borrow();
        let page = self
            .shared
            .ui()
            .map(|ui| format!("{:?}", ui.get_page()))
            .unwrap_or_default();
        format!(
            "page={page} wifi={:?} ha={:?} url={:?} pages={} states={} heap_internal={} heap_psram={}",
            st.connection.as_ref().map(|c| (&c.ssid, c.ip, c.rssi)),
            st.ha_status,
            st.ha_url,
            st.layout.pages.len(),
            st.states.len(),
            unsafe {
                esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_INTERNAL)
            },
            unsafe {
                esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_SPIRAM)
            },
        )
    }
}

impl Shared {
    fn ui(&self) -> Option<AppWindow> {
        self.ui.upgrade()
    }

    fn page(&self) -> Page {
        self.ui().map(|ui| ui.get_page()).unwrap_or(Page::Welcome)
    }

    fn set_page(&self, page: Page) {
        if let Some(ui) = self.ui() {
            ui.set_page(page);
        }
    }

    fn register_callbacks(self: &Rc<Self>, ui: &AppWindow) {
        macro_rules! on {
            ($method:ident, |$s:ident $(, $arg:ident)*| $body:expr) => {{
                let $s = self.clone();
                ui.$method(move |$($arg),*| $body);
            }};
        }

        // ---- Wi-Fi wizard
        on!(on_start_setup, |s| s.show_networks());
        on!(on_rescan, |s| s.wifi.send(Command::Scan));
        on!(on_choose_network, |s, item| {
            {
                let mut st = s.st.borrow_mut();
                st.selected_ssid = item.ssid.to_string();
                st.security_known = true;
                st.selected_open = !item.secure;
            }
            if item.secure {
                s.show_entry(Entry::WifiPassword, String::new());
            } else {
                s.connect_wifi(String::new());
            }
        });
        on!(on_choose_hidden, |s| s
            .show_entry(Entry::WifiSsid, String::new()));
        on!(on_change_network, |s| s.show_networks());
        on!(on_forget_network, |s| s.forget_wifi());

        // ---- Text entry
        on!(on_key, |s, text| s.type_text(&text));
        on!(on_backspace, |s| {
            s.st.borrow_mut().entry.pop();
            s.refresh_entry();
        });
        on!(on_toggle_reveal, |s| {
            {
                let mut st = s.st.borrow_mut();
                st.revealed = !st.revealed;
            }
            s.refresh_entry();
        });
        on!(on_submit_entry, |s| s.submit_entry());
        on!(on_entry_back, |s| s.entry_back());

        // ---- Progress and result pages
        on!(on_status_action, |s| {
            let progress = s.st.borrow().progress;
            if matches!(progress, Progress::Wifi { .. }) {
                s.wifi.send(Command::Cancel);
                s.show_networks();
            }
        });
        on!(on_result_primary_clicked, |s| {
            let outcome = s.st.borrow().outcome;
            match outcome {
                Outcome::WifiOk => s.after_wifi(),
                Outcome::WifiFailed => s.retry_wifi(),
                Outcome::HaOk => s.show_dashboard(),
                Outcome::HaFailed => s.show_entry(Entry::HaPassword, String::new()),
            }
        });
        on!(on_result_secondary_clicked, |s| {
            let outcome = s.st.borrow().outcome;
            match outcome {
                Outcome::WifiFailed => s.show_networks(),
                Outcome::HaFailed => s.show_ha_find(),
                Outcome::WifiOk | Outcome::HaOk => {}
            }
        });

        // ---- Home Assistant setup
        on!(on_ha_search, |s| s.discover());
        on!(on_ha_choose, |s, inst| {
            let user = {
                let mut st = s.st.borrow_mut();
                st.ha_url_input = inst.url.to_string();
                st.ha_user.clone()
            };
            s.show_entry(Entry::HaUser, user);
        });
        on!(on_ha_manual, |s| {
            let current = s.st.borrow().ha_url_input.clone();
            let prefill = if current.is_empty() {
                "http://".to_string()
            } else {
                current
            };
            s.show_entry(Entry::HaUrl, prefill);
        });
        on!(on_ha_find_back, |s| {
            let target = s.st.borrow().ha_find_return;
            if target == Page::Networks {
                s.show_networks();
            } else {
                s.set_page(target);
            }
        });
        on!(on_ha_sign_in, |s| s.show_ha_find());
        on!(on_ha_sign_out, |s| s.ha.send(ha::Command::Logout));

        // ---- Dashboard
        on!(on_open_settings, |s| {
            s.refresh_settings();
            s.set_page(Page::Settings);
        });
        on!(on_page_selected, |s, index| {
            s.st.borrow_mut().page_index = index.max(0) as usize;
            s.refresh_dashboard();
        });
        on!(on_dash_message_action, |s| {
            let action = s.st.borrow().dash_action;
            match action {
                DashAction::SetUp => s.show_ha_find(),
                DashAction::SignIn => {
                    let user = s.st.borrow().ha_user.clone();
                    s.show_entry(Entry::HaUser, user);
                }
                DashAction::None => {}
            }
        });
        on!(on_tile_tapped, |s, id| s.tile_tapped(&id, false));
        on!(on_tile_long_pressed, |s, id| s.tile_tapped(&id, true));
        on!(on_climate_step, |s, direction| s.climate_step(direction));
        on!(on_climate_set_mode, |s, mode| {
            let id = s.st.borrow().climate_tile.clone();
            if let Some(id) = id {
                s.action(&id, "set_mode", Some(json!(mode.as_str())));
            }
        });
        on!(on_dimmer_commit, |s, value| {
            let id = s.st.borrow().dimmer_tile.clone();
            if let Some(id) = id {
                s.st.borrow_mut().dimmer_touched = Some(Instant::now());
                s.action(&id, "brightness", Some(json!(value)));
            }
        });
        on!(on_dimmer_toggle, |s| {
            let id = s.st.borrow().dimmer_tile.clone();
            if let Some(id) = id {
                s.action(&id, "toggle", None);
            }
        });
    }

    // ------------------------------------------------------------ pages ----

    fn show_networks(&self) {
        self.set_page(Page::Networks);
        self.wifi.send(Command::Scan);
    }

    fn show_progress(&self, title: &str, detail: &str, button: &str) {
        if let Some(ui) = self.ui() {
            ui.set_status_title(title.into());
            ui.set_status_detail(detail.into());
            ui.set_status_retrying(false);
            ui.set_status_button(button.into());
            ui.set_page(Page::Connecting);
        }
    }

    fn show_result(
        &self,
        outcome: Outcome,
        title: &str,
        detail: &str,
        primary: &str,
        secondary: &str,
    ) {
        self.st.borrow_mut().outcome = outcome;
        if let Some(ui) = self.ui() {
            let ok = matches!(outcome, Outcome::WifiOk | Outcome::HaOk);
            ui.set_result_ok(ok);
            ui.set_result_title(title.into());
            ui.set_result_detail(detail.into());
            ui.set_result_primary(primary.into());
            ui.set_result_secondary(secondary.into());
            ui.set_result_auto(ok);
            ui.set_page(Page::Result);
        }
    }

    fn show_ha_find(&self) {
        let back = match self.page() {
            Page::Settings | Page::Dashboard => Page::Settings,
            _ => Page::Networks,
        };
        self.st.borrow_mut().ha_find_return = back;
        self.set_page(Page::HaFind);
        self.discover();
    }

    fn discover(&self) {
        if let Some(ui) = self.ui() {
            ui.set_ha_searching(true);
        }
        self.instances.set_vec(Vec::new());
        self.ha.send(ha::Command::Discover);
    }

    fn show_dashboard(&self) {
        self.refresh_dashboard();
        self.set_page(Page::Dashboard);
    }

    /// Where to go once Wi-Fi is up.
    fn after_wifi(&self) {
        if self.st.borrow().ha_url.is_some() {
            self.show_dashboard();
        } else {
            self.show_ha_find();
        }
    }

    // ------------------------------------------------------- text entry ----

    fn show_entry(&self, mode: Entry, prefill: String) {
        let (ssid, url, user, known) = {
            let mut st = self.st.borrow_mut();
            st.entry_mode = mode;
            st.entry = prefill;
            st.revealed = !matches!(mode, Entry::WifiPassword | Entry::HaPassword);
            (
                st.selected_ssid.clone(),
                st.ha_url_input.clone(),
                st.ha_user.clone(),
                st.security_known,
            )
        };
        let wifi_step = "Step 1 of 2 · Wi-Fi";
        let ha_step = "Step 2 of 2 · Home Assistant";
        let url = ha::auth::normalize_url(&url);
        let (title, subtitle, placeholder, enter, toggle) = match mode {
            Entry::WifiSsid => (
                "Network name".to_string(),
                format!("{wifi_step} · Enter the exact name (SSID)"),
                "Network name",
                "Next",
                false,
            ),
            Entry::WifiPassword => (
                format!("Password for {ssid}"),
                format!(
                    "{wifi_step} · {}",
                    if known {
                        "At least 8 characters"
                    } else {
                        "Leave empty for an open network"
                    }
                ),
                "Password",
                "Connect",
                true,
            ),
            Entry::HaUrl => (
                "Home Assistant address".to_string(),
                format!("{ha_step} · For example http://192.168.1.10:8123"),
                "http://",
                "Next",
                false,
            ),
            Entry::HaUser => (
                "Sign in to Home Assistant".to_string(),
                format!("{ha_step} · {url}"),
                "Username",
                "Next",
                false,
            ),
            Entry::HaPassword => (
                format!("Password for {user}"),
                format!("{ha_step} · {url}"),
                "Password",
                "Sign in",
                true,
            ),
            Entry::HaMfa => (
                "Two-factor code".to_string(),
                format!("{ha_step} · Enter the code from your authenticator app"),
                "Code",
                "Verify",
                false,
            ),
        };
        if let Some(ui) = self.ui() {
            ui.set_entry_title(title.into());
            ui.set_entry_subtitle(subtitle.into());
            ui.set_entry_placeholder(placeholder.into());
            ui.set_entry_enter_label(enter.into());
            ui.set_entry_toggle(toggle);
            ui.set_page(Page::Entry);
        }
        self.refresh_entry();
    }

    fn type_text(&self, text: &str) {
        {
            let mut st = self.st.borrow_mut();
            let max = st.entry_mode.max_chars();
            let bytes = st.entry_mode == Entry::WifiSsid;
            for c in text.chars() {
                let fits = if bytes {
                    st.entry.len() + c.len_utf8() <= max
                } else {
                    st.entry.chars().count() < max
                };
                if fits {
                    st.entry.push(c);
                }
            }
        }
        self.refresh_entry();
    }

    fn entry_valid(&self) -> bool {
        let st = self.st.borrow();
        let len = st.entry.chars().count();
        let text = st.entry.trim();
        match st.entry_mode {
            Entry::WifiSsid => !text.is_empty(),
            Entry::WifiPassword if st.security_known => len >= MIN_WIFI_PASSWORD,
            Entry::WifiPassword => len == 0 || len >= MIN_WIFI_PASSWORD,
            Entry::HaUrl => {
                let host = text
                    .trim_start_matches("https://")
                    .trim_start_matches("http://");
                !host.is_empty()
            }
            Entry::HaUser | Entry::HaPassword | Entry::HaMfa => !text.is_empty(),
        }
    }

    fn refresh_entry(&self) {
        let Some(ui) = self.ui() else { return };
        let valid = self.entry_valid();
        let st = self.st.borrow();
        let display = if st.revealed {
            st.entry.clone()
        } else {
            "●".repeat(st.entry.chars().count())
        };
        ui.set_entry_display(display.into());
        ui.set_entry_revealed(st.revealed);
        ui.set_entry_valid(valid);
    }

    fn submit_entry(&self) {
        if !self.entry_valid() {
            return;
        }
        let (mode, entry) = {
            let mut st = self.st.borrow_mut();
            (st.entry_mode, std::mem::take(&mut st.entry))
        };
        match mode {
            Entry::WifiSsid => {
                {
                    let mut st = self.st.borrow_mut();
                    st.selected_ssid = entry;
                    st.security_known = false;
                    st.selected_open = false;
                }
                self.show_entry(Entry::WifiPassword, String::new());
            }
            Entry::WifiPassword => self.connect_wifi(entry),
            Entry::HaUrl => {
                let user = {
                    let mut st = self.st.borrow_mut();
                    st.ha_url_input = ha::auth::normalize_url(&entry);
                    st.ha_user.clone()
                };
                self.show_entry(Entry::HaUser, user);
            }
            Entry::HaUser => {
                self.st.borrow_mut().ha_user = entry.trim().to_string();
                self.show_entry(Entry::HaPassword, String::new());
            }
            Entry::HaPassword => {
                let (url, username) = {
                    let mut st = self.st.borrow_mut();
                    st.progress = Progress::HaLogin;
                    (st.ha_url_input.clone(), st.ha_user.clone())
                };
                self.show_progress("Signing in…", &ha::auth::normalize_url(&url), "");
                self.ha.send(ha::Command::Login {
                    url,
                    username,
                    password: entry,
                });
            }
            Entry::HaMfa => {
                self.st.borrow_mut().progress = Progress::HaLogin;
                self.show_progress("Checking the code…", "", "");
                self.ha.send(ha::Command::SubmitMfa {
                    code: entry.trim().to_string(),
                });
            }
        }
    }

    fn entry_back(&self) {
        let mode = self.st.borrow().entry_mode;
        match mode {
            Entry::WifiSsid | Entry::WifiPassword => self.set_page(Page::Networks),
            Entry::HaUrl | Entry::HaUser => self.set_page(Page::HaFind),
            Entry::HaPassword => {
                let user = self.st.borrow().ha_user.clone();
                self.show_entry(Entry::HaUser, user);
            }
            Entry::HaMfa => self.show_entry(Entry::HaPassword, String::new()),
        }
    }

    // ------------------------------------------------------------- Wi-Fi ----

    fn connect_wifi(&self, password: String) {
        let ssid = {
            let mut st = self.st.borrow_mut();
            st.last_password = password.clone();
            st.progress = Progress::Wifi { boot: false };
            st.selected_ssid.clone()
        };
        self.show_progress(
            &format!("Connecting to {ssid}…"),
            "This can take a few seconds.",
            "Cancel",
        );
        self.wifi.send(Command::Connect { ssid, password });
    }

    fn retry_wifi(&self) {
        let (open, password) = {
            let st = self.st.borrow();
            (st.selected_open, st.last_password.clone())
        };
        if open {
            // Nothing to correct for an open network: just try again.
            self.connect_wifi(String::new());
        } else {
            self.show_entry(Entry::WifiPassword, password);
        }
    }

    fn forget_wifi(&self) {
        self.wifi.send(Command::Forget);
        {
            let mut st = self.st.borrow_mut();
            st.connection = None;
            st.last_password.clear();
        }
        self.networks.set_vec(Vec::new());
        if let Some(ui) = self.ui() {
            ui.set_wifi_online(false);
            ui.set_page(Page::Welcome);
        }
    }

    fn on_wifi_event(&self, event: wifi::Event) {
        let Some(ui) = self.ui() else { return };
        let progress = self.st.borrow().progress;
        let waiting =
            ui.get_page() == Page::Connecting && matches!(progress, Progress::Wifi { .. });
        match event {
            wifi::Event::ScanStarted => ui.set_scanning(true),
            wifi::Event::ScanDone(found) => {
                let items: Vec<NetworkItem> = found
                    .iter()
                    .map(|n| NetworkItem {
                        ssid: n.ssid.as_str().into(),
                        bars: wifi::bars(n.rssi),
                        secure: n.secure,
                    })
                    .collect();
                self.networks.set_vec(items);
                ui.set_scan_message(
                    "No networks found. Move closer to the router, or enter the name by hand."
                        .into(),
                );
                ui.set_scanning(false);
            }
            wifi::Event::ScanFailed(reason) => {
                ui.set_scan_message(format!("Couldn't scan for networks ({reason}).").into());
                ui.set_scanning(false);
            }
            wifi::Event::Connecting { ssid, attempt } => {
                if waiting {
                    ui.set_status_title(format!("Connecting to {ssid}…").into());
                    if attempt > 1 {
                        ui.set_status_detail(format!("Attempt {attempt}…").into());
                    }
                }
            }
            wifi::Event::Connected(connection) => {
                ui.set_wifi_online(true);
                ui.set_wifi_ssid(connection.ssid.as_str().into());
                ui.set_ip_address(connection.ip.to_string().into());
                ui.set_wifi_bars(wifi::bars(connection.rssi));
                ui.set_wifi_status(rssi_text(connection.rssi));
                let detail = format!("{} · IP address {}", connection.ssid, connection.ip);
                self.st.borrow_mut().connection = Some(connection);
                self.ha.send(ha::Command::NetworkUp);
                if waiting {
                    let boot = progress == Progress::Wifi { boot: true };
                    let ha_ready = self.st.borrow().ha_url.is_some();
                    if boot && ha_ready {
                        self.show_dashboard();
                    } else {
                        self.show_result(Outcome::WifiOk, "Connected", &detail, "Continue", "");
                    }
                }
                self.refresh_dashboard();
            }
            wifi::Event::Retrying {
                ssid,
                reason,
                delay,
            } => {
                ui.set_wifi_online(false);
                ui.set_wifi_status(format!("Reconnecting… {}", reason.describe()).into());
                if waiting {
                    ui.set_status_retrying(true);
                    ui.set_status_title(format!("Can't reach {ssid}").into());
                    ui.set_status_detail(
                        format!(
                            "{} Trying again in {} s.",
                            reason.describe(),
                            delay.as_secs()
                        )
                        .into(),
                    );
                    ui.set_status_button("Choose another network".into());
                }
                self.refresh_dashboard();
            }
            wifi::Event::Failed { ssid, reason } => {
                self.st.borrow_mut().connection = None;
                ui.set_wifi_online(false);
                if waiting {
                    let mut detail = reason.describe();
                    if reason == FailReason::WrongPassword {
                        detail.push_str(" Passwords are case-sensitive.");
                    }
                    self.show_result(
                        Outcome::WifiFailed,
                        &format!("Couldn't connect to {ssid}"),
                        &detail,
                        "Try again",
                        "Choose another network",
                    );
                }
                self.refresh_dashboard();
            }
            wifi::Event::Idle => {
                self.st.borrow_mut().connection = None;
                ui.set_wifi_online(false);
                self.refresh_dashboard();
            }
            wifi::Event::Rssi(rssi) => {
                ui.set_wifi_bars(wifi::bars(rssi));
                ui.set_wifi_status(rssi_text(rssi));
                if let Some(c) = self.st.borrow_mut().connection.as_mut() {
                    c.rssi = rssi;
                }
            }
        }
    }

    // --------------------------------------------------- Home Assistant ----

    fn on_ha_event(&self, event: ha::Event) {
        let Some(ui) = self.ui() else { return };
        match event {
            ha::Event::Discovered(found) => {
                let items: Vec<InstanceItem> = found
                    .iter()
                    .map(|i| InstanceItem {
                        name: i.name.as_str().into(),
                        url: i.url.as_str().into(),
                    })
                    .collect();
                self.instances.set_vec(items);
                ui.set_ha_searching(false);
            }
            ha::Event::LoginFailed(message) => self.show_result(
                Outcome::HaFailed,
                "Couldn't sign in",
                &message,
                "Try again",
                "Change address",
            ),
            ha::Event::MfaRequired => self.show_entry(Entry::HaMfa, String::new()),
            ha::Event::LoggedIn => {
                let name = {
                    let mut st = self.st.borrow_mut();
                    st.ha_url = Some(ha::auth::normalize_url(&st.ha_url_input));
                    st.layout = Layout::default();
                    st.states.clear();
                    st.page_index = 0;
                    st.screen_name.clone()
                };
                self.refresh_settings();
                self.show_result(
                    Outcome::HaOk,
                    "Signed in to Home Assistant",
                    &format!(
                        "This screen shows up in Home Assistant as \u{201c}{name}\u{201d}. Design its layout under Screens in the sidebar."
                    ),
                    "Continue",
                    "",
                );
            }
            ha::Event::LoggedOut => {
                {
                    let mut st = self.st.borrow_mut();
                    st.ha_url = None;
                    st.layout = Layout::default();
                    st.states.clear();
                }
                self.refresh_settings();
                self.refresh_dashboard();
                if ui.get_page() == Page::Settings {
                    self.show_ha_find();
                }
            }
            ha::Event::Status(status) => {
                log::info!("Home Assistant: {status:?}");
                self.st.borrow_mut().ha_status = status;
                self.refresh_settings();
                self.refresh_dashboard();
            }
            ha::Event::ScreenName(name) => {
                ui.set_screen_name(name.as_str().into());
                self.st.borrow_mut().screen_name = name;
            }
            ha::Event::Layout(layout) => {
                {
                    let mut st = self.st.borrow_mut();
                    if st.page_index >= layout.pages.len() {
                        st.page_index = 0;
                    }
                    st.layout = layout;
                }
                self.refresh_dashboard();
            }
            ha::Event::States(states) => {
                let changed: Vec<String> = states.iter().map(|(id, _)| id.clone()).collect();
                self.st.borrow_mut().states.extend(states);
                self.update_tiles(&changed);
                self.refresh_popups();
            }
            ha::Event::ActionFailed(message) => ui.set_toast(message.into()),
        }
    }

    fn on_update(&self, status: crate::ota::Status) {
        use crate::ota::Status;
        let Some(ui) = self.ui() else { return };
        match status {
            Status::Started { source, version } => {
                ui.set_climate_open(false);
                ui.set_dimmer_open(false);
                ui.set_update_failed(false);
                ui.set_update_progress(0.0);
                ui.set_update_title(format!("Updating to {version}").into());
                ui.set_update_detail(
                    format!("Installing from {source}. Don't unplug the screen.").into(),
                );
                ui.set_updating(true);
            }
            Status::Writing(pct) => ui.set_update_progress(pct as f32 / 100.0),
            Status::Verifying => ui.set_update_detail("Checking the new firmware…".into()),
            Status::Restarting => {
                ui.set_update_progress(1.0);
                ui.set_update_detail("Restarting…".into());
            }
            Status::Failed(error) => {
                ui.set_update_failed(true);
                ui.set_update_title("Update failed".into());
                ui.set_update_detail(
                    format!("{error}. The screen keeps running {}.", crate::ota::VERSION).into(),
                );
                ui.set_updating(true);
            }
        }
    }

    fn refresh_settings(&self) {
        let Some(ui) = self.ui() else { return };
        let st = self.st.borrow();
        ui.set_ha_configured(st.ha_url.is_some());
        ui.set_ha_url(st.ha_url.clone().unwrap_or_default().into());
        let status = match &st.ha_status {
            Status::NotConfigured => "Not set up".to_string(),
            Status::Connecting => "Connecting…".to_string(),
            Status::Connected => "Connected".to_string(),
            Status::Offline(reason) => format!("Offline · {reason}"),
            Status::NeedsLogin(_) => "Signed out, sign in again".to_string(),
            Status::NotInstalled => "Integration not installed".to_string(),
        };
        ui.set_ha_status(status.into());
    }

    // -------------------------------------------------------- dashboard ----

    fn refresh_dashboard(&self) {
        let Some(ui) = self.ui() else { return };
        let mut st = self.st.borrow_mut();

        let wifi_up = st.connection.is_some();
        let has_pages = !st.layout.pages.is_empty();
        let (title, detail, button, action) = if st.ha_url.is_none() {
            (
                "Home Assistant isn't set up".to_string(),
                "Connect this screen to Home Assistant to show your devices.".to_string(),
                "Set up",
                DashAction::SetUp,
            )
        } else {
            match &st.ha_status {
                Status::NeedsLogin(reason) => (
                    "Sign in again".to_string(),
                    reason.clone(),
                    "Sign in",
                    DashAction::SignIn,
                ),
                Status::NotInstalled => (
                    "Install the ESP32 Screen integration".to_string(),
                    "In Home Assistant open Settings › Devices & services › Add integration, and add ESP32 Screen.".to_string(),
                    "",
                    DashAction::None,
                ),
                Status::Connected if !has_pages => (
                    "Nothing to show yet".to_string(),
                    format!(
                        "In Home Assistant, open Screens in the sidebar and design the layout for \u{201c}{}\u{201d}.",
                        st.screen_name
                    ),
                    "",
                    DashAction::None,
                ),
                Status::Offline(reason) if !has_pages => (
                    "Can't reach Home Assistant".to_string(),
                    reason.clone(),
                    "",
                    DashAction::None,
                ),
                _ if !has_pages => (
                    "Connecting to Home Assistant…".to_string(),
                    if wifi_up {
                        String::new()
                    } else {
                        "Waiting for Wi-Fi.".to_string()
                    },
                    "",
                    DashAction::None,
                ),
                _ => (String::new(), String::new(), "", DashAction::None),
            }
        };
        st.dash_action = action;
        ui.set_dash_message_title(title.into());
        ui.set_dash_message_detail(detail.into());
        ui.set_dash_message_button(button.into());

        let chip = if !wifi_up {
            "Wi-Fi offline"
        } else {
            match st.ha_status {
                Status::Offline(_) if has_pages => "Home Assistant offline",
                Status::Connecting if has_pages => "Connecting…",
                _ => "",
            }
        };
        ui.set_dash_status(chip.into());

        let tabs: Vec<PageTab> = st
            .layout
            .pages
            .iter()
            .map(|p| PageTab {
                title: p.title.as_str().into(),
                icon: crate::icons::page_glyph(&p.icon).into(),
            })
            .collect();
        ui.set_tabs_compact(tabs_too_wide(&tabs));
        self.pages.set_vec(tabs);
        ui.set_dash_page(st.page_index as i32);

        let tiles: Vec<TileData> = match st.layout.pages.get(st.page_index) {
            Some(page) => {
                ui.set_grid_cols(page.cols.max(1) as i32);
                ui.set_grid_rows(page.rows.max(1) as i32);
                page.tiles
                    .iter()
                    .map(|t| tile_data(t, st.states.get(&t.id)))
                    .collect()
            }
            None => Vec::new(),
        };
        drop(st);
        self.tiles.set_vec(tiles);
    }

    /// Updates the tiles on the current page whose state changed.
    fn update_tiles(&self, changed: &[String]) {
        let st = self.st.borrow();
        let Some(page) = st.layout.pages.get(st.page_index) else {
            return;
        };
        for (row, tile) in page.tiles.iter().enumerate() {
            if changed.contains(&tile.id) && row < self.tiles.row_count() {
                self.tiles
                    .set_row_data(row, tile_data(tile, st.states.get(&tile.id)));
            }
        }
    }

    fn find_tile(&self, id: &str) -> Option<(Tile, TileState)> {
        let st = self.st.borrow();
        let tile = st
            .layout
            .pages
            .iter()
            .flat_map(|p| p.tiles.iter())
            .find(|t| t.id == id)?
            .clone();
        let state = st.states.get(id).cloned().unwrap_or_default();
        Some((tile, state))
    }

    fn action(&self, tile_id: &str, action: &'static str, value: Option<serde_json::Value>) {
        self.ha.send(ha::Command::Action {
            tile_id: tile_id.to_string(),
            action,
            value,
        });
    }

    fn tile_tapped(&self, id: &str, long: bool) {
        let Some((tile, state)) = self.find_tile(id) else {
            return;
        };
        if !state.available {
            if let Some(ui) = self.ui() {
                ui.set_toast(format!("{} is unavailable.", tile.label).into());
            }
            return;
        }
        match tile.kind.as_str() {
            "climate" => self.open_climate(&tile.id),
            "light" if long && state.brightness >= 0 => self.open_dimmer(&tile.id),
            _ if long => {}
            _ => self.action(&tile.id, "toggle", None),
        }
    }

    // ----------------------------------------------------------- pop-ups ----

    fn open_climate(&self, id: &str) {
        {
            let mut st = self.st.borrow_mut();
            st.climate_tile = Some(id.to_string());
            st.pending_target = None;
        }
        self.refresh_popups();
        if let Some(ui) = self.ui() {
            ui.set_climate_open(true);
        }
    }

    fn open_dimmer(&self, id: &str) {
        {
            let mut st = self.st.borrow_mut();
            st.dimmer_tile = Some(id.to_string());
            st.dimmer_touched = None;
        }
        self.refresh_popups();
        if let Some(ui) = self.ui() {
            ui.set_dimmer_open(true);
        }
    }

    fn climate_step(&self, direction: i32) {
        let id = self.st.borrow().climate_tile.clone();
        let Some((_, state)) = id.and_then(|id| self.find_tile(&id)) else {
            return;
        };
        let step = state.step.unwrap_or(0.5).max(0.1);
        {
            let mut st = self.st.borrow_mut();
            let base = st
                .pending_target
                .map(|p| p.0)
                .or(state.target)
                .unwrap_or(21.0);
            let mut value = base + direction as f64 * step;
            if let Some(min) = state.min {
                value = value.max(min);
            }
            if let Some(max) = state.max {
                value = value.min(max);
            }
            let now = Instant::now();
            st.pending_target = Some((
                value,
                Some(now + TEMPERATURE_DEBOUNCE),
                now + TEMPERATURE_HOLD,
            ));
        }
        self.refresh_popups();
    }

    /// Debounced work: sends a pending temperature, drops stale holds.
    fn tick(&self) {
        let now = Instant::now();
        let mut send = None;
        let mut refresh = false;
        {
            let mut st = self.st.borrow_mut();
            if let Some((value, send_at, hold)) = st.pending_target {
                if send_at.is_some_and(|at| now >= at) {
                    send = st.climate_tile.clone().map(|id| (id, value));
                    st.pending_target = Some((value, None, now + TEMPERATURE_HOLD));
                } else if send_at.is_none() && now >= hold {
                    st.pending_target = None;
                    refresh = true;
                }
            }
        }
        if let Some((id, value)) = send {
            self.action(&id, "set_temperature", Some(json!(value)));
        }
        if refresh {
            self.refresh_popups();
        }
    }

    fn refresh_popups(&self) {
        let Some(ui) = self.ui() else { return };
        let climate_id = self.st.borrow().climate_tile.clone();
        if let Some((tile, state)) = climate_id.and_then(|id| self.find_tile(&id)) {
            let pending = {
                let mut st = self.st.borrow_mut();
                // Home Assistant reached the value we sent: stop overriding.
                if let (Some((value, None, _)), Some(target)) = (st.pending_target, state.target) {
                    if (value - target).abs() < 0.01 {
                        st.pending_target = None;
                    }
                }
                st.pending_target.map(|p| p.0)
            };
            let step = state.step.unwrap_or(0.5);
            let decimals = if step < 1.0 { 1 } else { 0 };
            let target = pending.or(state.target);
            ui.set_climate_name(tile.label.as_str().into());
            ui.set_climate_target(
                target
                    .map(|t| format!("{t:.decimals$}"))
                    .unwrap_or_default()
                    .into(),
            );
            ui.set_climate_current(
                state
                    .current
                    .map(|c| format!("{c:.1}"))
                    .unwrap_or_default()
                    .into(),
            );
            ui.set_climate_humidity(
                state
                    .humidity
                    .map(|h| format!("{h:.0}"))
                    .unwrap_or_default()
                    .into(),
            );
            ui.set_climate_unit(state.unit.as_str().into());
            ui.set_climate_mode(state.mode.as_str().into());
            ui.set_climate_available(state.available);
            ui.set_climate_can_down(
                target
                    .zip(state.min)
                    .map(|(t, m)| t > m + 0.001)
                    .unwrap_or(target.is_some()),
            );
            ui.set_climate_can_up(
                target
                    .zip(state.max)
                    .map(|(t, m)| t < m - 0.001)
                    .unwrap_or(target.is_some()),
            );
            let modes: Vec<ModeItem> = state
                .modes
                .iter()
                .map(|m| ModeItem {
                    id: m.as_str().into(),
                    label: mode_label(m).into(),
                })
                .collect();
            if self
                .modes
                .iter()
                .map(|m| m.id)
                .ne(modes.iter().map(|m| m.id.clone()))
            {
                self.modes.set_vec(modes);
            }
        }

        let dimmer_id = self.st.borrow().dimmer_tile.clone();
        if let Some((tile, state)) = dimmer_id.and_then(|id| self.find_tile(&id)) {
            ui.set_dimmer_name(tile.label.as_str().into());
            ui.set_dimmer_on(state.on);
            // Don't fight the finger: skip state echoes right after a change.
            let recently = self
                .st
                .borrow()
                .dimmer_touched
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            if !recently && state.on && state.brightness >= 0 {
                ui.set_dimmer_value(state.brightness.max(1));
            }
        }
    }
}

fn tile_data(tile: &Tile, state: Option<&TileState>) -> TileData {
    let fallback = TileState {
        status: "…".into(),
        ..TileState::default()
    };
    let s = state.unwrap_or(&fallback);
    TileData {
        id: tile.id.as_str().into(),
        kind: tile.kind.as_str().into(),
        domain: tile.entity_id.split('.').next().unwrap_or("").into(),
        label: tile.label.as_str().into(),
        x: tile.x as i32,
        y: tile.y as i32,
        w: tile.w.max(1) as i32,
        h: tile.h.max(1) as i32,
        available: s.available,
        on: s.on,
        status: s.status.as_str().into(),
        detail: s.detail.as_str().into(),
        moving: s.moving,
        mode: s.mode.as_str().into(),
        value: big_value(tile, s).into(),
    }
}

/// Large temperature for climate tiles; the UI shows it on tall tiles only.
fn big_value(tile: &Tile, s: &TileState) -> String {
    if tile.kind != "climate" || !s.available {
        return String::new();
    }
    match s.target {
        _ if s.mode == "off" => "Off".into(),
        Some(t) => {
            let decimals = if s.step.unwrap_or(0.5) < 1.0 { 1 } else { 0 };
            format!("{t:.decimals$}°")
        }
        None => String::new(),
    }
}

/// Rough check whether all tab titles fit in the top bar (800 px minus the
/// Wi-Fi icon, settings button and padding). Inter at 20 px averages about
/// 11 px per character.
fn tabs_too_wide(tabs: &[PageTab]) -> bool {
    const AVAILABLE: usize = 660;
    let needed: usize = tabs
        .iter()
        .map(|t| {
            let icon = if t.icon.is_empty() { 0 } else { 34 };
            6 + 38 + icon + 11 * t.title.chars().count()
        })
        .sum();
    needed > AVAILABLE
}

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "off" => "Off",
        "heat" => "Heat",
        "cool" => "Cool",
        "heat_cool" => "Heat/Cool",
        "auto" => "Auto",
        "dry" => "Dry",
        "fan_only" => "Fan",
        _ => "Other",
    }
}

fn rssi_text(rssi: i8) -> SharedString {
    let quality = match wifi::bars(rssi) {
        4 => "Excellent",
        3 => "Good",
        2 => "Fair",
        _ => "Weak",
    };
    format!("{quality} signal · {rssi} dBm").into()
}
