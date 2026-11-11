# esp32-screen

Rust firmware for an ESP32-S3 800x480 touch screen (Sunton ESP32-8048S043 class:
N16R8 module, parallel RGB panel, GT911 touch, SD slot).

On boot it shows a two-step setup wizard, then a dashboard designed in Home
Assistant.

1. **Wi-Fi.** Network list, password on an on-screen keyboard, connection
   progress, then success or the reason for a failure. With a saved network the
   wizard opens on the connection step and keeps retrying with backoff.
2. **Home Assistant.** Instances are found with mDNS, or the address is typed
   in. The screen signs in with a Home Assistant username and password,
   including two-factor codes, through Home Assistant's own login flow. Only the
   address and a refresh token are stored, never the password.
3. **Dashboard.** Pages of tiles on a grid, as laid out in the **Screens**
   editor in Home Assistant. Tap a light, gate, garage door or valve to toggle
   it. Tap a toggle to enable or disable an automation, or to switch an
   input_boolean or switch. Long-press a light for its controls: brightness,
   colour and colour temperature, whichever it supports. Per light tile, the
   editor can make a tap open those controls instead of switching the light
   (so a stray tap can't turn it off), and put brightness and colour sliders
   on the tile itself. Tiles of coloured lights glow in the light's colour. Tap an AC tile for target temperature and mode. AC tiles can take
   their temperature and humidity from separate sensors, chosen in the editor.
   Scene tiles run a scene, script or button. Sensor tiles show a value. Blind,
   lock, alarm and media tiles open a panel with their controls, including a
   keypad when a lock or alarm needs a code. Layout changes saved in the
   editor appear immediately. Each page has a tab with an optional icon,
   chosen from 46 room and function icons in the editor. Light tiles can
   show a lamp of their own instead of the bulb (ceiling, pendant, wall
   lantern, floodlight, LED strip and more, 31 in all). When the titles
   don't fit, the other pages show only their icons. The top bar shows the
   Wi-Fi signal next to the gear icon, which opens settings: change or forget
   Wi-Fi, sign out of Home Assistant, and the screensaver.
4. **Screensaver (optional).** A plain clock or a flip clock with the date,
   a 7-day weather forecast, or both taking turns every 10, 20, 30 or 60
   seconds. It starts after 1, 5 or 15 minutes without touches on the
   dashboard, with the backlight dimmed. The clock can show the weather and
   an indoor temperature, in English or Portuguese, with a 12- or 24-hour
   clock. The forecast comes from the weather entity picked for the screen. The touch that wakes
   the screen doesn't press anything. Time comes from NTP and the timezone
   from Home Assistant. Backlight and screensaver can also be controlled from
   Home Assistant.
5. **Notifications.** Home Assistant can show a message on the screen, with
   a camera snapshot or another picture. The screen wakes up for it. A tap
   closes it, and so do its timeout and Home Assistant.

With an SD card in, the screen keeps the last layout and tile states on it.
After a restart the dashboard appears right away, before Home Assistant
answers, and stays visible while Home Assistant is offline.

Wi-Fi credentials are saved only after a connection with them succeeds. If new
credentials fail, the screen goes back to the saved network. To start over,
use **Forget** in settings or hold the **BOOT** button for 5 seconds.

## Layout

| Path | What |
|---|---|
| `src/board.rs` | Pin map and panel timings, recovered from the factory firmware |
| `src/display.rs` | RGB panel through ESP-IDF `esp_lcd`, backlight PWM |
| `src/touch.rs` | GT911 driver |
| `src/wifi.rs` | Wi-Fi worker thread: scan, connect, retry, NVS credentials |
| `src/ha/` | Home Assistant worker: login (`auth.rs`), websocket (`ws.rs`), SD cache (`cache.rs`), mDNS discovery, protocol |
| `src/app.rs` | Wizard and dashboard state, UI callbacks, worker events to pages |
| `src/console.rs` | Serial debug console (taps, typing, screenshots), development builds only |
| `src/ota.rs` | Firmware updates: install, rollback check, `update.bin` from the SD card |
| `src/logic/` | Code without hardware dependencies (clock text, touch filter, URLs), tested on a computer |
| `src/sd.rs` | microSD card on SPI, mounted as FAT at `/sdcard` |
| `src/icons.rs` | The page and light tile icons the firmware can draw (must match the editor's lists) |
| `src/saver.rs` | Screensaver: idle timer, clock digits, flip animation, settings |
| `ui/clock/*.png` | Clock digits, rendered by `scripts/gen_clock_digits.py` |
| `ui/*.slint` | All screens ([Slint](https://slint.dev), software renderer) |
| `docs/protocol.md` | Contract between the firmware and the integration |
| `custom_components/esp32_screen/` | The Home Assistant integration and its editor panel |
| `host-tests/` | Tests for `src/logic/`, the tile model and UI consistency, run on a computer |
| `homeassistant/` | Dev Home Assistant (Docker), simulator and tests for the integration |
| `scripts/` | Build wrapper, serial bridge, flash backup |

## Build and flash

Requires the Xtensa Rust toolchain (`espup install`), `ldproxy` and `espflash`.
ESP-IDF v5.5.3 is fetched into `~/.espressif` on the first build.

```sh
scripts/flash.sh                       # build and flash over USB
scripts/build-firmware.sh              # build dist/esp32-screen-<version>.bin
scripts/build-firmware.sh --publish homeassistant/dev-config/esp32_screen/firmware
scripts/test-host.sh                   # host tests, with standard Rust
```

`scripts/flash.sh` builds with the debug console. Images from
`scripts/build-firmware.sh` leave it out unless given `--dev`.

Always build through `scripts/cargo-esp.sh`, which the other scripts use. It
selects the `esp` toolchain, because Homebrew's `rustc` shadows rustup's on
this machine. It also passes the absolute path of `partitions.csv` to ESP-IDF.

`scripts/flash.sh` flashes the bootloader ESP-IDF builds, which supports
rollback (espflash's bundled one doesn't). It also erases `otadata`, so the
board boots the flashed image even after an over-the-air update moved it to
the other slot. Wi-Fi and Home Assistant settings survive. The first time
after the factory firmware, also erase `nvs` and `phy_init`
(`--erase-parts nvs,otadata,phy_init`): the factory firmware's settings live
where this firmware's `nvs` partition is. The CH340 serial bridge is
unreliable above 230400 baud through a USB dock.

## Firmware updates

The version comes from `Cargo.toml`; bump it for every release.

- **Over Wi-Fi.** Copy `dist/esp32-screen-<version>.bin` into Home
  Assistant's `<config>/esp32_screen/firmware/` folder. Each screen's
  *Firmware* update entity offers it within a minute. Install it from
  Settings › Updates or the device page; the screen shows the progress.
- **From the SD card.** Copy the image to the card as `update.bin`. At boot
  the screen installs it if its version differs from the running one, then
  renames it `update.bin.installed` (or `.failed`). A 5 MB image takes about
  40 s from the card, about 20 s over Wi-Fi.
- **Rollback.** The new firmware has to run for 60 s before it is kept. If it
  crashes or resets earlier, the bootloader starts the previous firmware
  again, which reports the failure to Home Assistant.

## Restore the factory firmware

`backup/` holds a full 16 MB dump of the flash as shipped. It is not in git.

```sh
uvx esptool --port /dev/cu.usbserial-XXXX --baud 230400 write-flash 0 backup/factory-firmware-16MB.bin
```

## Debug console

Development builds (the default `dev-console` feature) read commands on the
serial port: `tap X Y [MS]`, `type TEXT`, `key enter`, `key back`, `shot`,
`status`, `sd`, `sdformat ERASE`, `sdcopyslot`, `saver`, `bench`,
`flipshot MS`, `page NAME`, `forget`, `reboot`. `src/console.rs` describes
each one. Logging pauses while `shot` sends a screenshot, so events during
those few seconds don't show in the log. Release images leave the console out, so anyone with a USB cable
can't drive the screen.

On macOS, opening the CH340 port resets the board. So `scripts/devctl.py serve`
opens it once and keeps it open, and the other subcommands go through it:

```sh
uv run --with pyserial --with pillow scripts/devctl.py serve &
uv run --with pyserial --with pillow scripts/devctl.py shot screen.png
uv run --with pyserial --with pillow scripts/devctl.py cmd "tap 158 354" 2
uv run --with pyserial --with pillow scripts/devctl.py log 5
```

Stop the server before flashing.

## Home Assistant integration

`custom_components/esp32_screen/` is the Home Assistant side: it
stores each screen's layout, adds a **Screens** panel with a visual grid editor,
and streams tile states to the screens. The contract with the firmware is
[`docs/protocol.md`](docs/protocol.md). Tested on Home Assistant 2026.11.1.

1. **Install.** With [HACS](https://hacs.xyz): HACS → ⋮ → Custom repositories →
   add `https://github.com/cascalheira/home-assistant-screen-esp32`, type
   *Integration*, then download *ESP32 Screen* and restart Home Assistant.
   Without HACS: copy `custom_components/esp32_screen` into your Home Assistant
   config directory as `custom_components/esp32_screen` and restart.
2. **Add the integration.** Settings → Devices & services → Add integration →
   *ESP32 Screen*. There is nothing to configure, and only one instance.
3. **Create a user for each screen.** Settings → People → Users → Add user, with
   *Administrator* off. Give every screen its own non-admin user. The screen
   only needs to read states and run the actions on its tiles. The editor
   commands reject non-admin users.
4. **Design the layout.** Open **Screens** in the sidebar (admins only). A screen
   shows up after it logs in and registers. To prepare a layout before that,
   add the screen by its MAC address. Pick a page (up to 6, 1-8 × 1-6 cells),
   click an empty cell to add a tile, drag tiles to move them, drag the corner
   to resize, then press **Save**. Connected screens update right away.
   Under **Screen settings**, pick a weather entity and an indoor temperature
   sensor for the clock screensaver, the language (English or Portuguese) and
   12- or 24-hour time.
   Tile kinds are light, gate, garage, valve, climate, toggle, scene, sensor,
   blind, lock, alarm and media. A toggle tile enables or disables an automation,
   or switches an input_boolean or switch. A scene tile runs a scene, script or
   button. A sensor tile only shows a value. Lock and alarm tiles ask for a code
   on the screen when the entity uses one. A climate tile can show a room
   temperature or humidity sensor instead of the AC's own reading.

Each screen also appears as a device (manufacturer "ESP32 Screen"). Deleting
that device deletes the screen's layout. The device has entities for the
backlight (a light), the screensaver mode and delay, buttons to wake the
screen or show the clock, and a sensor that is on while the screensaver
shows. Use them in automations, for example to dim the screens at night.

**Notifications.** The `esp32_screen.notify` action shows a message on one,
several or all screens. It can include a camera snapshot or an image entity,
and can stay up for a number of seconds or until it is dismissed with
`esp32_screen.dismiss`. For example, a doorbell automation can send the
camera picture with a 60 s timeout.

**Firmware updates.** Copy a firmware image (`.bin`, project `esp32-screen`) into
`<HA config>/esp32_screen/firmware/`. Within a minute every screen's
*Firmware* update entity offers the highest version in that folder. Install it
from Settings → Updates or from the screen's device page. The screen must be
online. It downloads the image, checks its size and sha256, and restarts. If the
new firmware does not start, the screen rolls back to the previous one. Old
images can stay in the folder. Remove an image to stop offering it. See
[`docs/protocol.md`](docs/protocol.md) section 5.

### Development

```sh
homeassistant/dev/run-ha.sh                                  # HA in Docker, port 8123, demo entities
uv run --with aiohttp homeassistant/dev/onboard.py           # owner + non-admin screen user, random passwords
uv run --with aiohttp homeassistant/tests/e2e.py             # protocol end-to-end test (uses device_sim.py)
uv run --with aiohttp homeassistant/tests/device_sim.py --interactive   # simulated screen AABBCCDDEEFF
uv run --with aiohttp homeassistant/tests/device_sim.py --ota apply --sw-version 0.1.0 --duration 600  # accepts updates
uv run --with aiohttp --with playwright homeassistant/tests/panel_browser.py  # editor in Chromium, /tmp/ha-editor-*.png
```

User names and passwords of the dev instance, and optionally the MAC of a real
screen for the tests, live in `homeassistant/dev-config/dev-local.json` (not in
git). `onboard.py` creates it with random passwords; environment variables can
override every value (see `homeassistant/dev/local.py`).

From the screen, use the development machine's LAN address. If it has several
network interfaces, use the one on the screen's network: a reply sent from
another interface may never reach the screen.

The integration is bind-mounted read-only into the container. After a change,
run `docker restart esp32-screen-ha`. Config and state live in
`homeassistant/dev-config/`, which git ignores. Firmware images for the dev
instance go in `homeassistant/dev-config/esp32_screen/firmware/`.

CI (`.github/workflows/integration.yml`) runs ruff, hassfest, the HACS check and
`e2e.py` against a fresh Home Assistant in Docker. Run `uvx ruff check
custom_components homeassistant` locally before pushing.
`.github/workflows/firmware.yml` runs the host tests and builds the release
image, which it keeps as a build artifact.

## Notes

- **Slint licence.** Slint is GPLv3, or royalty-free for desktop, mobile and
  web, or commercial. Embedded devices are not covered by the royalty-free
  licence. Check this before shipping the firmware outside the GPL.
- **Fonts.** Inter (SIL Open Font License) and Material Design Icons
  (Pictogrammers Free License), see `ui/fonts/`. Glyphs are embedded as
  signed distance fields: one per character, scaled to any font size, so
  sizes cost almost no flash. But the fields are made at the largest size
  used per font, so keep text and icons at 48 px or less: one 76 px icon and
  56 px text cost about 465 KB. Bitmaps per size took about 1.7 MB more and redrew
  only 5-10% faster. Text covers ASCII and Latin-1, which includes Portuguese
  accents. Other scripts in names or SSIDs show missing glyphs. To add
  characters, extend the `glyph-coverage` text in `ui/app.slint`.
- **Rendering.** Double buffered: Slint draws line by line into internal RAM,
  lines are copied into the back buffer in PSRAM, and the driver switches
  buffers at the end of a frame, so no half-drawn frame reaches the panel.
  The UI runs on one core and the panel's interrupts on the other. A full
  redraw takes about 100 ms for the dashboard and 85 ms for settings.
- **Memory.** The firmware (about 4.4 MB) runs from PSRAM, because running
  from flash hung the board. That leaves about 2.2 MB of PSRAM free next to
  the two frame buffers. A notification picture needs up to about 430 KB,
  and is left out when that would leave less than 96 KB. Making the firmware
  smaller frees PSRAM too.
- **Notification pictures** are decoded by the JPEG decoder in the chip's
  ROM (Espressif's `esp_jpeg` component). Pure-Rust decoders did not fit:
  one starts a thread for every picture, another needs about 29 KB of stack
  and hung on this chip.
- **Clock digits are images.** Single-colour PNGs, stored as alpha maps of
  about 17 KB each, scale smoothly for the flip animation.
- **Compiler workaround.** Deserializing `f32` with serde crashes the Xtensa
  LLVM backend ("Cannot select ... PCREL_WRAPPER"), so tile temperatures are
  `f64`. The same bug hits Slint code like `opacity: c ? 1 : 0.4`, a choice
  between two float constants. Use colours or other expressions instead.
- **Websocket quirk.** Home Assistant's greeting arrives in the same packet as
  the handshake, and ESP-IDF's transport only releases it after its read
  timeout. The screen therefore sends its auth message as soon as it connects.
- **HTTPS.** An `https://` address works with a certificate from a public
  authority (Let's Encrypt, Nabu Casa). With a self-signed certificate the
  screen shows its SHA-256 fingerprint and asks whether to trust it. Compare
  it with `openssl x509 -noout -fingerprint -sha256 -in <certificate>`. A
  trusted certificate is the only one accepted for that server from then on.
  If it changes, the screen stops and asks you to sign in again, so the new
  one gets checked. TLS buffers live in PSRAM: internal RAM has about 30 KB
  free, less than one TLS session needs.
- **Security.** The Wi-Fi password is stored in NVS without flash encryption.
- **SD card.** The card is mounted at boot. It holds `update.bin` for updates
  and `esp32-screen/cache.json`, the last layout and states. The cache has
  the Home Assistant address and the names of the entities on the dashboard,
  but no credentials, and is deleted on sign-out. Settings shows the free
  space. FAT with long file names is supported. exFAT cards must be
  reformatted: the console command `sdformat ERASE` replaces all partitions
  with one FAT32 partition over the whole card, then restarts.
- **No booting from SD.** The ESP32-S3's ROM bootloader can only load from SPI
  flash, or receive code over UART/USB in download mode. Firmware updates from
  the card or over Wi-Fi go through the normal two OTA slots instead.
- **Pins in use.** GPIO19/20 are the native USB pins; this board routes them to
  the touch controller, so native USB stays disabled.
