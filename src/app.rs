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

use crate::ha::model::{Forecast, Layout, Tile, TileState};
use crate::ha::{self, HaControl, HaHandle, Status};
use crate::logic::clock;
use crate::wifi::{self, Command, Connection, FailReason, WifiControl, WifiHandle};
use crate::{
    ActionButton, AppWindow, ForecastDayView, InstanceItem, ModeItem, NetworkItem, Page, PageTab,
    TileData,
};

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
    /// Home Assistant's certificate needs the user's decision.
    HaUntrusted,
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
    forecast: Forecast,
    portuguese: bool,
    page_index: usize,
    dash_action: DashAction,

    // Pop-ups
    climate_tile: Option<String>,
    /// Temperature chosen with +/-: (value, send at, hold display until).
    /// `send at` is None once sent.
    pending_target: Option<(f64, Option<Instant>, Instant)>,
    dimmer_tile: Option<String>,
    /// Tile shown in the action sheet (blind, lock, alarm, media) and the
    /// code typed on its keypad.
    sheet_tile: Option<String>,
    sheet_code: String,
    sheet_touched: Option<Instant>,

    /// Notification on screen: its id and when it closes (None = until
    /// tapped or dismissed).
    notify_id: Option<String>,
    notify_until: Option<Instant>,
    /// The doorbell ring on screen ("" for a console test) and when it closes.
    doorbell_ring: Option<String>,
    doorbell_until: Option<Instant>,
    /// Last time the doorbell video kept the screen awake.
    doorbell_woke: Option<Instant>,
    pin_flow: Option<PinFlow>,
    /// Digits typed on the PIN pad.
    pin_entry: String,
}

impl State {
    /// What the action sheet's slider controls for the open tile.
    fn sheet_slider_action(&self) -> Option<&'static str> {
        let id = self.sheet_tile.as_ref()?;
        let tile = self
            .layout
            .pages
            .iter()
            .flat_map(|p| p.tiles.iter())
            .find(|t| &t.id == id)?;
        match tile.kind.as_str() {
            "blind" => Some("position"),
            "media" => Some("volume"),
            _ => None,
        }
    }
}

pub struct App {
    shared: Rc<Shared>,
    /// Display commands for the main loop (it owns the backlight and the
    /// screensaver).
    screen_commands: Rc<RefCell<Vec<crate::saver::ScreenCommand>>>,
    /// Set when Home Assistant (re)subscribes: the screen state must be sent.
    resubscribed: Rc<std::cell::Cell<bool>>,
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
    sheet_buttons: Rc<VecModel<ActionButton>>,
    st: RefCell<State>,
    /// Shared with `App::screen_commands`.
    screen_commands: Rc<RefCell<Vec<crate::saver::ScreenCommand>>>,
    lock: RefCell<crate::lock::Lock>,
}

/// What the PIN pad is for.
#[derive(Clone, Debug, PartialEq)]
enum PinFlow {
    UnlockSettings,
    UnlockScreens,
    NewFirst,
    NewConfirm(String),
}

impl App {
    pub fn new(
        ui: &AppWindow,
        wifi: WifiHandle,
        ha: HaHandle,
        updates: Receiver<crate::ota::Status>,
        lock: crate::lock::Lock,
    ) -> Self {
        let shared = Rc::new(Shared {
            lock: RefCell::new(lock),
            ui: ui.as_weak(),
            wifi: wifi.control,
            ha: ha.control(),
            networks: Rc::new(VecModel::default()),
            instances: Rc::new(VecModel::default()),
            pages: Rc::new(VecModel::default()),
            tiles: Rc::new(VecModel::default()),
            modes: Rc::new(VecModel::default()),
            sheet_buttons: Rc::new(VecModel::default()),
            screen_commands: Rc::default(),
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
                forecast: Forecast::default(),
                portuguese: false,
                page_index: 0,
                dash_action: DashAction::None,
                climate_tile: None,
                pending_target: None,
                dimmer_tile: None,
                sheet_tile: None,
                sheet_code: String::new(),
                sheet_touched: None,
                notify_id: None,
                notify_until: None,
                doorbell_ring: None,
                doorbell_until: None,
                doorbell_woke: None,
                pin_flow: None,
                pin_entry: String::new(),
            }),
        });

        ui.set_networks(ModelRc::from(shared.networks.clone()));
        ui.set_ha_instances(ModelRc::from(shared.instances.clone()));
        ui.set_dash_pages(ModelRc::from(shared.pages.clone()));
        ui.set_tiles(ModelRc::from(shared.tiles.clone()));
        ui.set_climate_modes(ModelRc::from(shared.modes.clone()));
        ui.set_sheet_buttons(ModelRc::from(shared.sheet_buttons.clone()));
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
            screen_commands: shared.screen_commands.clone(),
            shared,
            resubscribed: Rc::default(),
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
            match event {
                ha::Event::Command(crate::saver::ScreenCommand::ClearPin) => {
                    self.shared.clear_pin()
                }
                ha::Event::Command(command) => self.screen_commands.borrow_mut().push(command),
                ha::Event::Settings { portuguese, h24 } => {
                    self.screen_commands
                        .borrow_mut()
                        .push(crate::saver::ScreenCommand::ClockFormat { portuguese, h24 });
                    self.shared.st.borrow_mut().portuguese = portuguese;
                    self.shared.show_forecast();
                }
                ha::Event::Info {
                    sun,
                    sunrise,
                    sunset,
                    ..
                } => {
                    self.screen_commands
                        .borrow_mut()
                        .push(crate::saver::ScreenCommand::Sun {
                            up: sun,
                            rise: sunrise,
                            set: sunset,
                        });
                    self.shared.on_ha_event(event);
                }
                ha::Event::Forecast(forecast) => {
                    self.screen_commands.borrow_mut().push(
                        crate::saver::ScreenCommand::ForecastReady(!forecast.days.is_empty()),
                    );
                    self.shared.st.borrow_mut().forecast = forecast;
                    self.shared.show_forecast();
                }
                ha::Event::Doorbell { .. } => {
                    self.screen_commands
                        .borrow_mut()
                        .push(crate::saver::ScreenCommand::Attention);
                    self.shared.on_ha_event(event);
                }
                ha::Event::DoorbellFrame(_) => {
                    // Keep the screen awake while the video plays.
                    let wake = {
                        let mut st = self.shared.st.borrow_mut();
                        let due = st
                            .doorbell_woke
                            .is_none_or(|at| at.elapsed() >= Duration::from_secs(5));
                        if due {
                            st.doorbell_woke = Some(Instant::now());
                        }
                        due
                    };
                    if wake {
                        self.screen_commands
                            .borrow_mut()
                            .push(crate::saver::ScreenCommand::Wake);
                    }
                    self.shared.on_ha_event(event);
                }
                ha::Event::Notify { .. } => {
                    // Wake the screen (and its backlight) for it.
                    self.screen_commands
                        .borrow_mut()
                        .push(crate::saver::ScreenCommand::Attention);
                    self.shared.on_ha_event(event);
                }
                ha::Event::Status(Status::Connected) => {
                    self.resubscribed.set(true);
                    self.shared
                        .on_ha_event(ha::Event::Status(Status::Connected));
                }
                event => self.shared.on_ha_event(event),
            }
        }
        while let Ok(status) = self.updates.try_recv() {
            self.shared.on_update(status);
        }
        self.shared.tick();
    }

    pub fn forget_wifi(&self) {
        self.shared.forget_wifi();
    }

    pub fn clear_pin(&self) {
        self.shared.clear_pin();
    }

    /// The screensaver started or nobody touched the screen for a while:
    /// the PIN is needed again.
    pub fn relock(&self) {
        if self.shared.lock.borrow_mut().lock() {
            log::info!("locked");
        }
    }

    /// Display commands received since the last call.
    pub fn take_screen_commands(&self) -> Vec<crate::saver::ScreenCommand> {
        std::mem::take(&mut *self.screen_commands.borrow_mut())
    }

    /// True once after each (re)subscription to Home Assistant.
    pub fn take_resubscribed(&self) -> bool {
        self.resubscribed.replace(false)
    }

    /// Debug console: show a video from `url` as if the doorbell rang.
    pub fn test_doorbell(&self, url: String) {
        self.shared.ha.send(ha::Command::DoorbellTest(url));
    }

    pub fn report_state(&self, state: crate::saver::ScreenState) {
        self.shared.ha.send(ha::Command::ReportState(state));
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

    /// Queues a display command for the main loop.
    fn screen_command(&self, command: crate::saver::ScreenCommand) {
        self.screen_commands.borrow_mut().push(command);
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
        on!(on_notify_closed, |s| s.close_notification());
        on!(on_doorbell_closed, |s| s.close_doorbell());
        on!(on_display_brightness_changed, |s, v| s.screen_command(
            crate::saver::ScreenCommand::PreviewBrightness(v.clamp(1, 100) as u8)
        ));
        on!(on_display_brightness_committed, |s, v| s.screen_command(
            crate::saver::ScreenCommand::Brightness(v.clamp(1, 100) as u8)
        ));
        on!(on_display_theme_picked, |s, i| s.screen_command(
            crate::saver::ScreenCommand::Theme(match i {
                0 => crate::saver::ThemeMode::Auto,
                1 => crate::saver::ThemeMode::Light,
                _ => crate::saver::ThemeMode::Dark,
            })
        ));
        on!(on_restart, |_s| {
            log::warn!("restart from the settings page");
            unsafe { esp_idf_svc::sys::esp_restart() };
        });
        on!(on_doorbell_action_pressed, |s| {
            let ring = s.st.borrow().doorbell_ring.clone();
            if let Some(ring) = ring.filter(|r| !r.is_empty()) {
                if let Some(ui) = s.ui() {
                    ui.set_doorbell_action_state(1);
                }
                s.ha.send(ha::Command::DoorbellAction(ring));
            }
        });
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
                Outcome::HaUntrusted => {
                    let url = s.st.borrow().ha_url_input.clone();
                    s.st.borrow_mut().progress = Progress::HaLogin;
                    s.show_progress("Signing in…", &ha::auth::normalize_url(&url), "");
                    s.ha.send(ha::Command::TrustCertificate);
                }
            }
        });
        on!(on_result_secondary_clicked, |s| {
            let outcome = s.st.borrow().outcome;
            match outcome {
                Outcome::WifiFailed => s.show_networks(),
                Outcome::HaFailed => s.show_ha_find(),
                Outcome::HaUntrusted => {
                    s.ha.send(ha::Command::RejectCertificate);
                    s.show_ha_find();
                }
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
        on!(on_open_settings, |s| s.open_settings());
        on!(on_security_set_pin, |s| s.start_pin(PinFlow::NewFirst));
        on!(on_security_remove_pin, |s| s.clear_pin());
        on!(on_security_lock_screens_picked, |s, i| {
            s.lock.borrow_mut().set_lock_screens(i == 1);
            s.refresh_settings();
        });
        on!(on_pin_key, |s, digit| s.pin_key(&digit));
        on!(on_pin_backspace, |s| {
            s.st.borrow_mut().pin_entry.pop();
            s.show_pin_entry(None);
        });
        on!(on_pin_ok, |s| s.pin_ok());
        on!(on_pin_cancel, |s| {
            if s.st.borrow().pin_flow != Some(PinFlow::UnlockScreens) {
                s.close_pin();
            }
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
        on!(on_dimmer_commit, |s, value| s
            .dimmer_action("brightness", value));
        on!(on_dimmer_hue_commit, |s, value| s
            .dimmer_action("hue", value));
        on!(on_dimmer_kelvin_commit, |s, value| s
            .dimmer_action("kelvin", value));
        on!(on_tile_adjust, |s, id, what, value| {
            let action = match what.as_str() {
                "brightness" => "brightness",
                "hue" => "hue",
                "kelvin" => "kelvin",
                _ => return,
            };
            s.action(&id, action, Some(json!(value)));
        });
        on!(on_dimmer_toggle, |s| {
            let id = s.st.borrow().dimmer_tile.clone();
            if let Some(id) = id {
                s.action(&id, "toggle", None);
            }
        });
        on!(on_sheet_action, |s, id| s.sheet_action(&id));
        on!(on_sheet_slider_commit, |s, value| {
            let (tile, action) = {
                let st = s.st.borrow();
                let Some(tile) = st.sheet_tile.clone() else {
                    return;
                };
                (tile, st.sheet_slider_action())
            };
            if let Some(action) = action {
                s.st.borrow_mut().sheet_touched = Some(Instant::now());
                s.action(&tile, action, Some(json!(value)));
            }
        });
        on!(on_sheet_key, |s, key| {
            {
                let mut st = s.st.borrow_mut();
                if st.sheet_code.len() < 12 {
                    st.sheet_code.push_str(&key);
                }
            }
            s.refresh_popups();
        });
        on!(on_sheet_backspace, |s| {
            s.st.borrow_mut().sheet_code.pop();
            s.refresh_popups();
        });
        on!(on_sheet_closed, |s| {
            let mut st = s.st.borrow_mut();
            st.sheet_tile = None;
            st.sheet_code.clear();
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
            ha::Event::UntrustedCertificate(fingerprint) => {
                // 32 bytes in four lines of eight, so it wraps predictably.
                let lines: Vec<&str> = fingerprint
                    .as_bytes()
                    .chunks(24)
                    .map(|line| {
                        std::str::from_utf8(line)
                            .unwrap_or("")
                            .trim_end_matches(':')
                    })
                    .collect();
                self.show_result(
                    Outcome::HaUntrusted,
                    "Check the certificate",
                    &format!(
                        "Home Assistant's certificate isn't from a known authority. It may be self-signed. Trust it only if its SHA-256 fingerprint is:\n\n{}",
                        lines.join("\n")
                    ),
                    "Trust",
                    "Cancel",
                );
            }
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
            ha::Event::Info {
                condition,
                weather,
                indoor,
                ..
            } => {
                ui.set_clock_weather_condition(condition.into());
                ui.set_clock_weather_temperature(weather.into());
                ui.set_clock_indoor_temperature(indoor.into());
            }
            ha::Event::Notify {
                id,
                title,
                message,
                image,
                timeout_secs,
            } => {
                log::info!("notification {id}: {title}");
                match image {
                    Some(picture) => {
                        ui.set_notify_image(slint::Image::from_rgb8(picture));
                        ui.set_notify_has_image(true);
                    }
                    None => {
                        ui.set_notify_image(slint::Image::default());
                        ui.set_notify_has_image(false);
                    }
                }
                ui.set_notify_title(title.into());
                ui.set_notify_message(message.into());
                ui.set_notify_open(true);
                let mut st = self.st.borrow_mut();
                st.notify_id = Some(id);
                st.notify_until =
                    (timeout_secs > 0).then(|| Instant::now() + Duration::from_secs(timeout_secs));
            }
            ha::Event::Doorbell {
                ring_id,
                title,
                timeout_secs,
                action,
                has_video,
            } => {
                log::info!("doorbell: {title}");
                {
                    let mut st = self.st.borrow_mut();
                    st.doorbell_ring = Some(ring_id);
                    st.doorbell_until = Some(Instant::now() + Duration::from_secs(timeout_secs));
                }
                ui.set_climate_open(false);
                ui.set_dimmer_open(false);
                ui.set_sheet_open(false);
                ui.set_doorbell_title(title.into());
                ui.set_doorbell_action(action.into());
                ui.set_doorbell_action_state(0);
                ui.set_doorbell_frame(slint::Image::default());
                ui.set_doorbell_has_frame(false);
                ui.set_doorbell_status(
                    if has_video {
                        "Connecting to the camera…"
                    } else {
                        ""
                    }
                    .into(),
                );
                ui.set_doorbell_open(true);
            }
            ha::Event::DoorbellFrame(picture) => {
                if self.st.borrow().doorbell_ring.is_some() {
                    ui.set_doorbell_frame(slint::Image::from_rgb8(picture));
                    ui.set_doorbell_has_frame(true);
                    ui.set_doorbell_status("".into());
                }
                ha::DOORBELL_FRAME_SHOWN.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            ha::Event::DoorbellVideoLost(reason) => {
                if self.st.borrow().doorbell_ring.is_some() {
                    log::warn!("doorbell video lost: {reason}");
                    ui.set_doorbell_status(
                        if ui.get_doorbell_has_frame() {
                            "The video stopped"
                        } else {
                            "No video from the camera"
                        }
                        .into(),
                    );
                }
            }
            ha::Event::DoorbellActionDone(ok, message) => {
                ui.set_doorbell_action_state(if ok { 2 } else { 3 });
                if !ok {
                    ui.set_doorbell_status(message.into());
                }
            }
            ha::Event::Dismiss(id) => {
                let showing = self.st.borrow().notify_id.clone();
                if showing.is_some_and(|showing| id.is_empty() || showing == id) {
                    self.close_notification();
                }
            }
            // Routed to the main loop by App::poll.
            ha::Event::Command(_) | ha::Event::Settings { .. } | ha::Event::Forecast(_) => {}
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
        {
            let lock = self.lock.borrow();
            ui.set_security_has_pin(lock.has_pin());
            ui.set_security_lock_screens(lock.lock_screens());
        }
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
            "light" if (long || tile.tap_opens_controls()) && state.has_light_controls() => {
                self.open_dimmer(&tile.id)
            }
            // Taps that open controls never toggle, even without controls.
            "light" if tile.tap_opens_controls() => {}
            // Locks and alarm panels always go through the sheet: a stray tap
            // must never unlock a door or disarm the alarm.
            "lock" | "alarm" => self.open_sheet(&tile.id),
            "blind" | "media" if long => self.open_sheet(&tile.id),
            "sensor" => {}
            "scene" if !long => self.action(&tile.id, "activate", None),
            "media" => self.action(&tile.id, "play_pause", None),
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

    /// Puts the forecast into the screensaver's forecast view.
    fn show_forecast(&self) {
        let Some(ui) = self.ui() else { return };
        let st = self.st.borrow();
        let pt = st.portuguese;
        let today = crate::saver::local_time().map(|tm| {
            format!(
                "{:04}-{:02}-{:02}",
                tm.tm_year + 1900,
                tm.tm_mon + 1,
                tm.tm_mday
            )
        });
        let degrees = |t: Option<i64>| t.map(|t| format!("{t}°")).unwrap_or_default();
        let days: Vec<ForecastDayView> = st
            .forecast
            .days
            .iter()
            .map(|d| ForecastDayView {
                name: if today.as_deref() == Some(d.date.as_str()) {
                    clock::today(pt)
                } else {
                    clock::weekday_short(d.weekday, pt)
                }
                .into(),
                condition: d.condition.as_str().into(),
                high: degrees(d.high).into(),
                low: degrees(d.low).into(),
                precipitation: d
                    .precipitation
                    .filter(|&p| p > 0)
                    .map(|p| format!("{p}%"))
                    .unwrap_or_default()
                    .into(),
            })
            .collect();
        let (title, hint) = match (days.is_empty(), pt) {
            (false, _) => ("", ""),
            (true, false) => (
                "No weather forecast",
                "Pick a weather entity for this screen in Home Assistant: Screens › Screen settings.",
            ),
            (true, true) => (
                "Sem previsão do tempo",
                "Escolha uma entidade de meteorologia para este ecrã no Home Assistant: Screens › Screen settings.",
            ),
        };
        ui.set_forecast_days(ModelRc::new(VecModel::from(days)));
        ui.set_forecast_title(title.into());
        ui.set_forecast_hint(hint.into());
    }

    fn dimmer_action(&self, action: &'static str, value: i32) {
        let id = self.st.borrow().dimmer_tile.clone();
        if let Some(id) = id {
            self.action(&id, action, Some(json!(value)));
        }
    }

    fn open_dimmer(&self, id: &str) {
        self.st.borrow_mut().dimmer_tile = Some(id.to_string());
        self.refresh_popups();
        if let Some(ui) = self.ui() {
            ui.set_dimmer_open(true);
        }
    }

    fn open_sheet(&self, id: &str) {
        {
            let mut st = self.st.borrow_mut();
            st.sheet_tile = Some(id.to_string());
            st.sheet_code.clear();
            st.sheet_touched = None;
        }
        self.refresh_popups();
        if let Some(ui) = self.ui() {
            ui.set_sheet_open(true);
        }
    }

    fn sheet_action(&self, action: &str) {
        let Some(id) = self.st.borrow().sheet_tile.clone() else {
            return;
        };
        let Some((tile, state)) = self.find_tile(&id) else {
            return;
        };
        let action: &'static str = match action {
            "open" => "open",
            "close" => "close",
            "stop" => "stop",
            "lock" => "lock",
            "unlock" => "unlock",
            "arm_home" => "arm_home",
            "arm_away" => "arm_away",
            "arm_night" => "arm_night",
            "arm_vacation" => "arm_vacation",
            "disarm" => "disarm",
            "previous" => "previous",
            "play_pause" | "pause" => "play_pause",
            "next" => "next",
            "toggle" => "toggle",
            _ => return,
        };
        let coded = matches!(tile.kind.as_str(), "lock" | "alarm");
        let code = std::mem::take(&mut self.st.borrow_mut().sheet_code);
        if coded && state.code_required && code.is_empty() {
            if let Some(ui) = self.ui() {
                ui.set_toast("Enter the code first.".into());
            }
            return;
        }
        let value = (coded && !code.is_empty()).then(|| json!(code));
        self.action(&id, action, value);
        self.refresh_popups();
    }

    fn refresh_sheet(&self, ui: &AppWindow) {
        let id = self.st.borrow().sheet_tile.clone();
        let Some((tile, state)) = id.and_then(|id| self.find_tile(&id)) else {
            return;
        };
        let button = |id: &str, label: &str, primary: bool| ActionButton {
            id: id.into(),
            label: label.into(),
            primary,
        };
        let mut status = state.status.clone();
        if !state.detail.is_empty() {
            status = format!("{status} · {}", state.detail);
        }
        let (buttons, slider): (Vec<ActionButton>, Option<(&str, i32)>) = match tile.kind.as_str() {
            "blind" => (
                vec![
                    button("open", "Open", false),
                    button("stop", "Stop", false),
                    button("close", "Close", false),
                ],
                (state.position >= 0).then_some(("Position", state.position)),
            ),
            "lock" => (
                vec![
                    button("unlock", "Unlock", state.status == "Locked"),
                    button("lock", "Lock", state.status != "Locked"),
                ],
                None,
            ),
            "alarm" => {
                let mut buttons: Vec<ActionButton> = state
                    .modes
                    .iter()
                    .map(|m| {
                        let label = match m.as_str() {
                            "arm_home" => "Home",
                            "arm_away" => "Away",
                            "arm_night" => "Night",
                            "arm_vacation" => "Vacation",
                            _ => "Arm",
                        };
                        button(m, label, false)
                    })
                    .collect();
                buttons.push(button("disarm", "Disarm", state.on));
                (buttons, None)
            }
            "media" => (
                vec![
                    button("previous", "Previous", false),
                    // "pause" only picks the pause icon; both send play_pause.
                    button(
                        if state.on { "pause" } else { "play_pause" },
                        if state.on { "Pause" } else { "Play" },
                        true,
                    ),
                    button("next", "Next", false),
                    button("toggle", "Power", false),
                ],
                (state.volume >= 0).then_some(("Volume", state.volume)),
            ),
            _ => (Vec::new(), None),
        };
        let code = self.st.borrow().sheet_code.chars().count();
        ui.set_sheet_title(tile.label.as_str().into());
        ui.set_sheet_kind(tile.kind.as_str().into());
        ui.set_sheet_status(status.into());
        ui.set_sheet_keypad(matches!(tile.kind.as_str(), "lock" | "alarm") && state.code_required);
        ui.set_sheet_code("●".repeat(code).into());
        ui.set_sheet_slider(slider.is_some());
        if let Some((label, value)) = slider {
            ui.set_sheet_slider_label(label.into());
            // Don't fight the finger: skip state echoes right after a change.
            let recently = self
                .st
                .borrow()
                .sheet_touched
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            if !recently {
                ui.set_sheet_slider_value(value);
            }
        }
        let same = self.sheet_buttons.row_count() == buttons.len()
            && self
                .sheet_buttons
                .iter()
                .zip(&buttons)
                .all(|(a, b)| a == *b);
        if !same {
            self.sheet_buttons.set_vec(buttons);
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

    // ------------------------------------------------------------ PIN ----

    fn open_settings(&self) {
        if self.lock.borrow().settings_locked() {
            self.start_pin(PinFlow::UnlockSettings);
        } else {
            self.refresh_settings();
            self.set_page(Page::Settings);
        }
    }

    fn clear_pin(&self) {
        self.lock.borrow_mut().clear_pin();
        if self.st.borrow().pin_flow.is_some() {
            self.close_pin();
        }
        self.refresh_settings();
    }

    fn start_pin(&self, flow: PinFlow) {
        let Some(ui) = self.ui() else { return };
        let (title, message) = match &flow {
            PinFlow::UnlockSettings => ("Enter the PIN to open the settings", ""),
            PinFlow::UnlockScreens => ("Enter the PIN", ""),
            PinFlow::NewFirst => ("Choose a PIN", "4 to 8 digits."),
            PinFlow::NewConfirm(_) => ("Enter the new PIN again", ""),
        };
        ui.set_pin_title(title.into());
        ui.set_pin_message(message.into());
        ui.set_pin_error(false);
        ui.set_pin_cancellable(flow != PinFlow::UnlockScreens);
        {
            let mut st = self.st.borrow_mut();
            st.pin_flow = Some(flow);
            st.pin_entry.clear();
        }
        ui.set_pin_length(0);
        ui.set_pin_open(true);
    }

    fn close_pin(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.pin_flow = None;
            st.pin_entry.clear();
        }
        if let Some(ui) = self.ui() {
            ui.set_pin_open(false);
            ui.set_pin_length(0);
        }
    }

    /// Updates the dots; `error` replaces the message (in red).
    fn show_pin_entry(&self, error: Option<String>) {
        let Some(ui) = self.ui() else { return };
        ui.set_pin_length(self.st.borrow().pin_entry.len() as i32);
        if let Some(error) = error {
            ui.set_pin_message(error.into());
            ui.set_pin_error(true);
        } else if ui.get_pin_error() {
            ui.set_pin_error(false);
            ui.set_pin_message("".into());
        }
    }

    fn pin_key(&self, digit: &str) {
        {
            let mut st = self.st.borrow_mut();
            if st.pin_entry.len() >= crate::logic::pin::MAX_LEN {
                return;
            }
            st.pin_entry.push_str(digit);
        }
        self.show_pin_entry(None);
    }

    fn pin_ok(&self) {
        let (flow, entry) = {
            let st = self.st.borrow();
            (st.pin_flow.clone(), st.pin_entry.clone())
        };
        let Some(flow) = flow else { return };
        self.st.borrow_mut().pin_entry.clear();
        match flow {
            PinFlow::UnlockSettings | PinFlow::UnlockScreens => {
                let result = self.lock.borrow_mut().check(&entry);
                match result {
                    Ok(()) => {
                        self.close_pin();
                        if flow == PinFlow::UnlockSettings {
                            self.refresh_settings();
                            self.set_page(Page::Settings);
                        }
                    }
                    Err(crate::lock::Refused::Wrong(left)) => self.show_pin_entry(Some(format!(
                        "Wrong PIN. {left} {} left before a wait.",
                        if left == 1 { "try" } else { "tries" }
                    ))),
                    Err(crate::lock::Refused::Wait(secs)) => self.show_pin_entry(Some(format!(
                        "Too many wrong PINs. Try again in {secs} s."
                    ))),
                }
            }
            PinFlow::NewFirst => {
                if crate::logic::pin::valid(&entry) {
                    self.start_pin(PinFlow::NewConfirm(entry));
                } else {
                    self.show_pin_entry(Some("Use 4 to 8 digits.".into()));
                }
            }
            PinFlow::NewConfirm(first) => {
                if entry == first {
                    self.lock.borrow_mut().set_pin(&entry);
                    self.close_pin();
                    self.refresh_settings();
                } else {
                    self.start_pin(PinFlow::NewFirst);
                    self.show_pin_entry(Some("The two PINs were different. Choose a PIN.".into()));
                }
            }
        }
    }

    /// Shows the PIN pad over a locked dashboard, and leaves the settings
    /// once they lock again.
    fn enforce_lock(&self) {
        let Some(ui) = self.ui() else { return };
        let page = ui.get_page();
        let (settings_locked, screens_locked) = {
            let lock = self.lock.borrow();
            (lock.settings_locked(), lock.screens_locked())
        };
        let flow = self.st.borrow().pin_flow.clone();
        if page == Page::Settings && settings_locked && flow.is_none() {
            self.set_page(Page::Dashboard);
        }
        if ui.get_page() == Page::Dashboard && screens_locked {
            if flow != Some(PinFlow::UnlockScreens) {
                self.start_pin(PinFlow::UnlockScreens);
            }
        } else if flow == Some(PinFlow::UnlockScreens) {
            self.close_pin();
        }
    }

    fn close_doorbell(&self) {
        let was_open = {
            let mut st = self.st.borrow_mut();
            st.doorbell_until = None;
            st.doorbell_ring.take().is_some()
        };
        if was_open {
            self.ha.send(ha::Command::DoorbellClose);
        }
        if let Some(ui) = self.ui() {
            ui.set_doorbell_open(false);
            // Free the frame.
            ui.set_doorbell_frame(slint::Image::default());
            ui.set_doorbell_has_frame(false);
        }
    }

    fn close_notification(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.notify_id = None;
            st.notify_until = None;
        }
        if let Some(ui) = self.ui() {
            ui.set_notify_open(false);
            // Free the picture.
            ui.set_notify_image(slint::Image::default());
        }
    }

    /// Debounced work: sends a pending temperature, drops stale holds,
    /// closes a notification whose time is up.
    fn tick(&self) {
        self.enforce_lock();
        let now = Instant::now();
        let expired = self
            .st
            .borrow()
            .notify_until
            .is_some_and(|until| now >= until);
        if expired {
            self.close_notification();
        }
        let doorbell_over = self
            .st
            .borrow()
            .doorbell_until
            .is_some_and(|until| now >= until);
        if doorbell_over {
            self.close_doorbell();
        }
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
        self.refresh_sheet(&ui);
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
            ui.set_dimmer_can_toggle(!tile.no_toggle());
            // The sliders keep their own value while touched.
            ui.set_dimmer_value(state.brightness);
            ui.set_dimmer_hue(state.hue);
            let (low, high) = state.kelvin_range().unwrap_or((-1, -1));
            ui.set_dimmer_kelvin_min(low);
            ui.set_dimmer_kelvin_max(high);
            ui.set_dimmer_kelvin(state.kelvin.max(0));
            ui.set_dimmer_tint(
                light_color(&state)
                    .filter(|_| state.on)
                    .unwrap_or(slint::Color::from_rgb_u8(0xf5, 0xb8, 0x3d)),
            );
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
        // Alarm tiles have no climate mode; "triggered" picks the siren icon.
        mode: if tile.kind == "alarm" && s.status == "Triggered" {
            "triggered".into()
        } else {
            s.mode.as_str().into()
        },
        value: big_value(tile, s).into(),
        current: climate_text(tile, s, |s| s.current.map(crate::logic::climate::current)).into(),
        target: climate_text(tile, s, |s| {
            s.target.map(|t| crate::logic::climate::target(t, s.step))
        })
        .into(),
        humidity: climate_text(tile, s, |s| s.humidity.map(|h| format!("{h:.0}%"))).into(),
        icon: if tile.kind == "light" {
            crate::icons::light_glyph(&tile.icon).into()
        } else {
            "".into()
        },
        ctl_brightness: tile.inline_brightness() && s.brightness >= 0,
        ctl_hue: tile.inline_color() && s.hue >= 0,
        ctl_kelvin: tile.inline_color() && s.kelvin_range().is_some(),
        brightness: s.brightness.max(0),
        hue: s.hue.max(0),
        kelvin: s.kelvin.max(0),
        kelvin_min: s.kelvin_min,
        kelvin_max: s.kelvin_max,
        has_color: tile.kind == "light" && s.on && light_color(s).is_some(),
        color: light_color(s).unwrap_or_default(),
    }
}

/// The light's own colour, for its tile and controls. Near-black colours
/// (HA reports some at very low brightness) would hide the tile, so they
/// don't count.
fn light_color(s: &TileState) -> Option<slint::Color> {
    let (r, g, b) = s.color()?;
    (r.max(g).max(b) >= 64).then(|| slint::Color::from_rgb_u8(r, g, b))
}

/// Large value for climate and sensor tiles; the UI shows it on tall tiles
/// only.
/// A piece of a climate tile's text; "" on other tiles or when unknown.
fn climate_text(tile: &Tile, s: &TileState, f: impl Fn(&TileState) -> Option<String>) -> String {
    if tile.kind != "climate" || !s.available {
        return String::new();
    }
    f(s).unwrap_or_default()
}

fn big_value(tile: &Tile, s: &TileState) -> String {
    if !s.available {
        return String::new();
    }
    // Numeric sensors only: wording like "Detected" is too wide to be big.
    if tile.kind == "sensor" {
        return if tile.entity_id.starts_with("sensor.") {
            s.status.clone()
        } else {
            String::new()
        };
    }
    if tile.kind != "climate" {
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
