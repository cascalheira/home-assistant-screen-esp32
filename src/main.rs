#![allow(unexpected_cfgs)] // cfgs used inside esp_idf_svc::sys::esp_app_desc!
//! Firmware for an ESP32-S3 800x480 touch screen.
//!
//! On boot it shows a setup wizard: with no stored Wi-Fi credentials it
//! starts at the welcome page; with stored credentials it shows the
//! connection step while it reconnects. Holding the BOOT button for five
//! seconds erases the credentials and returns to the wizard.

mod app;
mod board;
mod console;
mod display;
mod ha;
mod icons;
mod logic;
mod ota;
mod saver;
mod sd;
mod touch;
mod wifi;

use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::ledc::{config::TimerConfig, LedcDriver, LedcTimerDriver, Resolution};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::FromValueType;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};

use board::{LCD_H_RES, LCD_V_RES};
use console::ConsoleCommand;
use display::{Backlight, Display};
use touch::{Sample, Touch};

slint::include_modules!();

// Version and project name in the image header, read by Home Assistant and
// the SD-card updater (see src/ota.rs).
esp_idf_svc::sys::esp_app_desc!();

const FACTORY_RESET_HOLD: Duration = Duration::from_secs(5);

struct EspPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl slint::platform::Platform for EspPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn debug_log(&self, arguments: core::fmt::Arguments) {
        log::info!("slint: {arguments}");
    }
}

/// Forwards an input event to Slint. Errors are logged, not fatal: a lost
/// pointer event must not take the UI down.
fn dispatch(window: &MinimalSoftwareWindow, event: WindowEvent) {
    if let Err(e) = window.dispatch_event_with_result(event) {
        log::warn!("dropped input event: {e:?}");
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    log::info!("esp32-screen {}", env!("CARGO_PKG_VERSION"));

    let p = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // Backlight stays off until the first frame is on the panel.
    let timer = LedcTimerDriver::new(
        p.ledc.timer0,
        &TimerConfig::new()
            .frequency(5.kHz().into())
            .resolution(Resolution::Bits10),
    )?;
    let channel = LedcDriver::new(p.ledc.channel0, &timer, p.pins.gpio2)?;
    let mut backlight = Backlight::new(channel, timer);
    backlight.set(0);

    // The panel's interrupt (which copies every frame from PSRAM into the
    // bounce buffers) runs on the core that creates the panel. Create it on
    // core 0, next to Wi-Fi, while this UI task runs on core 1
    // (CONFIG_ESP_MAIN_TASK_AFFINITY_CPU1).
    let mut display = {
        use esp_idf_svc::hal::cpu::Core;
        use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
        ThreadSpawnConfiguration {
            name: Some(c"lcd-init"),
            pin_to_core: Some(Core::Core0),
            ..Default::default()
        }
        .set()?;
        let created = thread::spawn(Display::new).join();
        ThreadSpawnConfiguration::default().set()?;
        created.map_err(|_| anyhow::anyhow!("display init thread panicked"))??
    };

    let i2c = I2cDriver::new(
        p.i2c0,
        p.pins.gpio19,
        p.pins.gpio20,
        &I2cConfig::new().baudrate(400.kHz().into()),
    )?;
    let mut touch = Touch::new(i2c, p.pins.gpio18, p.pins.gpio38)?;
    let boot_button = PinDriver::input(p.pins.gpio0, Pull::Up)?;

    let sd_card = match sd::SdCard::mount(
        p.spi2,
        p.pins.gpio12,
        p.pins.gpio11,
        p.pins.gpio13,
        p.pins.gpio10,
    ) {
        Ok(card) => Some(card),
        Err(e) => {
            log::info!("no SD card: {e:#}");
            None
        }
    };
    let sd_status = match sd_card.as_ref().and_then(|c| c.usage()) {
        Some((total, free)) => format!("SD card: {} free of {}", sd::size(free), sd::size(total)),
        None if sd_card.is_some() => "SD card mounted".to_string(),
        None => "No SD card (insert one and restart to use it)".to_string(),
    };

    let window = MinimalSoftwareWindow::new(RepaintBufferType::SwappedBuffers);
    window.set_size(PhysicalSize::new(LCD_H_RES as u32, LCD_V_RES as u32));
    slint::platform::set_platform(Box::new(EspPlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .map_err(|e| anyhow::anyhow!("slint platform: {e:?}"))?;

    let ui = AppWindow::new().map_err(|e| anyhow::anyhow!("create UI: {e:?}"))?;
    let wifi = wifi::start(p.modem, sysloop, nvs.clone())?;
    let screen_id = wifi.mac.replace(':', "");
    let (update_tx, update_rx) = mpsc::channel();
    let ha = ha::start(nvs.clone(), screen_id, update_tx.clone(), sd_card.is_some())?;
    let app = app::App::new(&ui, wifi, ha, update_rx);
    if sd_card.is_some() {
        ota::update_from_sd(update_tx);
    }
    let saver = saver::Saver::new(nvs, &ui);
    // Network time, started once Wi-Fi is up so the first request isn't lost
    // (lwIP waits a long time before retrying). The timezone comes from Home
    // Assistant (see ha/mod.rs).
    let mut sntp: Option<esp_idf_svc::sntp::EspSntp<'static>> = None;
    ui.set_sd_status(sd_status.as_str().into());
    ui.show()
        .map_err(|e| anyhow::anyhow!("show window: {e:?}"))?;

    let (console_tx, console_rx) = mpsc::channel();
    #[cfg(feature = "dev-console")]
    console::start(console_tx);
    #[cfg(not(feature = "dev-console"))]
    drop(console_tx);

    log::info!(
        "ready, touch {}",
        if touch.is_present() { "ok" } else { "missing" }
    );

    let mut pointer_down: Option<LogicalPosition> = None;
    let mut injected_release: Option<(LogicalPosition, Instant)> = None;
    let mut boot_pressed_since: Option<Instant> = None;
    let mut backlight_on = false;
    let started = Instant::now();
    let mut last_reported: Option<saver::ScreenState> = None;
    let mut marked_healthy = false;
    // One display line in internal RAM for the line-by-line renderer.
    let mut line_buffer = vec![Rgb565Pixel::default(); LCD_H_RES];

    loop {
        slint::platform::update_timers_and_animations();

        // ---- Touch
        match touch.poll() {
            Sample::Pressed { x, y } => {
                let position = LogicalPosition::new(x as f32, y as f32);
                let first = pointer_down.is_none();
                if first {
                    // Real finger-downs only; useful to tell real taps from
                    // phantom touches.
                    log::info!("touch down at {x},{y}");
                }
                let swallow = if first {
                    saver
                        .borrow_mut()
                        .on_press(&ui, &mut backlight, position.x, position.y)
                } else {
                    saver.borrow().swallowing()
                };
                let event = if swallow {
                    None
                } else if !first {
                    Some(WindowEvent::PointerMoved { position })
                } else {
                    Some(WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    })
                };
                if let Some(event) = event {
                    dispatch(&window, event);
                }
                pointer_down = Some(position);
            }
            Sample::Released => {
                let swallowed = saver.borrow_mut().on_release();
                if let Some(position) = pointer_down.take().filter(|_| !swallowed) {
                    dispatch(
                        &window,
                        WindowEvent::PointerReleased {
                            position,
                            button: PointerEventButton::Left,
                        },
                    );
                    dispatch(&window, WindowEvent::PointerExited);
                }
            }
            Sample::None => {}
        }

        // ---- Debug console
        if let Some((position, at)) = injected_release {
            if Instant::now() >= at {
                dispatch(
                    &window,
                    WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    },
                );
                dispatch(&window, WindowEvent::PointerExited);
                injected_release = None;
            }
        }
        while let Ok(command) = console_rx.try_recv() {
            match command {
                ConsoleCommand::Tap { x, y, ms } => {
                    let position = LogicalPosition::new(x as f32, y as f32);
                    if saver
                        .borrow_mut()
                        .on_press(&ui, &mut backlight, position.x, position.y)
                    {
                        // Woke the screen: like a finger, this tap goes no further.
                        saver.borrow_mut().on_release();
                        continue;
                    }
                    dispatch(&window, WindowEvent::PointerMoved { position });
                    dispatch(
                        &window,
                        WindowEvent::PointerPressed {
                            position,
                            button: PointerEventButton::Left,
                        },
                    );
                    injected_release =
                        Some((position, Instant::now() + Duration::from_millis(ms as u64)));
                }
                ConsoleCommand::Type(text) => ui.invoke_key(text.into()),
                ConsoleCommand::Enter => ui.invoke_submit_entry(),
                ConsoleCommand::Backspace => ui.invoke_backspace(),
                ConsoleCommand::Status => println!(
                    "STATUS {} frames={:?} saver={} time={}",
                    app.status(),
                    display::frame_stats(),
                    saver.borrow().is_active(),
                    saver::local_time()
                        .map(|t| format!("{:02}:{:02}:{:02}", t.tm_hour, t.tm_min, t.tm_sec))
                        .unwrap_or_else(|| "unset".into())
                ),
                ConsoleCommand::Saver => saver.borrow_mut().start_now(&ui, &mut backlight),
                ConsoleCommand::Bench => {
                    let mut times = Vec::new();
                    for _ in 0..5 {
                        window.request_redraw();
                        let t = Instant::now();
                        window.draw_if_needed(|renderer| {
                            // NewBuffer forces a full repaint.
                            renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
                            renderer.render_by_line(display::LineWriter {
                                target: display.back_buffer(),
                                line: &mut line_buffer,
                            });
                        });
                        times.push(t.elapsed().as_millis());
                        display.present();
                    }
                    // Back to partial repaints (this also forces one full repaint).
                    window.draw_if_needed(|renderer| {
                        renderer.set_repaint_buffer_type(RepaintBufferType::SwappedBuffers);
                    });
                    window.request_redraw();
                    println!("BENCH full redraw ms: {times:?}");
                }
                ConsoleCommand::FlipTest => saver.borrow_mut().test_flip(&ui),
                ConsoleCommand::Sd => match &sd_card {
                    Some(card) => println!(
                        "SD {sd_status}, card {}\n{}",
                        card.card_size().map(sd::size).unwrap_or_default(),
                        card.listing()
                    ),
                    None => println!("SD {sd_status}"),
                },
                ConsoleCommand::SdCopySlot => match ota::copy_spare_slot_to_sd() {
                    Ok(n) => println!("SDCOPY OK {n} bytes; restart to install it"),
                    Err(e) => println!("SDCOPY FAILED: {e:#}"),
                },
                ConsoleCommand::SdFormat => match &sd_card {
                    Some(card) => {
                        match card.format_whole_card() {
                            Ok(()) => println!("SDFORMAT OK, restarting"),
                            Err(e) => println!("SDFORMAT FAILED: {e:#}"),
                        }
                        thread::sleep(Duration::from_millis(500));
                        unsafe { esp_idf_svc::sys::esp_restart() };
                    }
                    None => println!("SDFORMAT no card mounted"),
                },
                ConsoleCommand::Page(name) => {
                    let page = match name.as_str() {
                        "welcome" => Some(Page::Welcome),
                        "networks" => Some(Page::Networks),
                        "entry" => Some(Page::Entry),
                        "connecting" => Some(Page::Connecting),
                        "result" => Some(Page::Result),
                        "ha-find" => Some(Page::HaFind),
                        "dashboard" => Some(Page::Dashboard),
                        "settings" => Some(Page::Settings),
                        _ => None,
                    };
                    match page {
                        Some(page) => ui.set_page(page),
                        None => println!("unknown page: {name}"),
                    }
                }
                ConsoleCommand::Forget => app.forget_wifi(),
            }
        }

        // ---- BOOT button: hold to forget Wi-Fi
        if boot_button.is_low() {
            let since = *boot_pressed_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= FACTORY_RESET_HOLD {
                log::warn!(
                    "BOOT held for {}s: forgetting Wi-Fi",
                    FACTORY_RESET_HOLD.as_secs()
                );
                app.forget_wifi();
                boot_pressed_since = Some(Instant::now() + Duration::from_secs(3600));
            }
        } else {
            boot_pressed_since = None;
        }

        // ---- Wi-Fi/HA events, screensaver and rendering
        app.poll();
        for command in app.take_screen_commands() {
            saver.borrow_mut().command(command, &ui, &mut backlight);
        }
        let state = saver.borrow().state();
        if app.take_resubscribed() || Some(state) != last_reported {
            app.report_state(state);
            last_reported = Some(state);
        }
        if sntp.is_none() && app.wifi_connected() {
            match esp_idf_svc::sntp::EspSntp::new_default() {
                Ok(s) => sntp = Some(s),
                Err(e) => log::warn!("SNTP: {e}"),
            }
        }
        saver.borrow_mut().tick(&ui, &mut backlight);
        if !marked_healthy && started.elapsed() >= ota::HEALTHY_AFTER {
            // New firmware that got this far stays installed (no rollback).
            ota::mark_healthy();
            marked_healthy = true;
        }
        let drawn = window.draw_if_needed(|renderer| {
            renderer.render_by_line(display::LineWriter {
                target: display.back_buffer(),
                line: &mut line_buffer,
            });
        });
        if drawn {
            display.present();
        }
        if !backlight_on {
            saver.borrow().apply_backlight(&mut backlight);
            backlight_on = true;
        }

        let idle = if window.has_active_animations() {
            Duration::from_millis(5)
        } else {
            slint::platform::duration_until_next_timer_update()
                .unwrap_or(Duration::MAX)
                .min(Duration::from_millis(15))
        };
        thread::sleep(idle.max(Duration::from_millis(1)));
    }
}
