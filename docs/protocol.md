# Screen ⇄ Home Assistant protocol

Two parts talk to each other:

- **Firmware** (`src/`) on each screen.
- **`esp32_screen` integration** (`custom_components/esp32_screen/`)
  in Home Assistant. It stores the layouts, serves the visual editor panel and
  turns entity states into small "tile view models" for the screens.

The screen logs in as a normal Home Assistant user (username + password through
HA's own auth flow, see below). All traffic after login uses HA's standard
websocket API at `/api/websocket`, with the commands defined here.

## 1. Login (firmware ⇄ HA core, no integration involved)

HA's OAuth-style login flow, the same one the frontend uses:

1. `POST /auth/login_flow`
   `{"client_id": CID, "handler": ["homeassistant", null], "redirect_uri": RURI}`
   → `{"type":"form","flow_id":...,"step_id":"init"}`
2. `POST /auth/login_flow/{flow_id}` `{"client_id": CID, "username": ..., "password": ...}`
   - `{"type":"create_entry","result":CODE}`: success.
   - `{"type":"form","step_id":"mfa", ...}`: post again with `{"client_id": CID, "code": "123456"}`.
   - `{"type":"form","errors":{"base":"invalid_auth"}}`: wrong credentials.
3. `POST /auth/token` (form-encoded) `grant_type=authorization_code&code=CODE&client_id=CID`
   → `{"access_token","refresh_token","expires_in":1800,"token_type":"Bearer"}`
4. Refresh: `POST /auth/token` `grant_type=refresh_token&refresh_token=RT&client_id=CID`.

`CID = http://esp32-screen-<MAC>.local/`, `RURI = CID + "auth_callback"`. HA
accepts a redirect URI on the same host as the client ID without fetching it.
The firmware stores the HA base URL and the refresh token in NVS, never the password.

## 2. Layout model

```jsonc
{
  "version": 1,
  "pages": [                       // 0-6 pages, shown as tabs on the screen
                                   // (0 = nothing configured yet)
    {
      "id": "p1",                  // stable string id
      "title": "Living room",      // 1-32 characters
      "icon": "mdi:sofa",          // "" (no icon) or one of the page icons below
      "cols": 4,                   // 1-8
      "rows": 3,                   // 1-6
      "tiles": [                   // at most cols*rows, never overlapping
        {
          "id": "t1",              // stable string id, unique within the layout
          "kind": "light",         // light | gate | garage | valve | climate | toggle |
                                   // scene | sensor | blind | lock | alarm | media
          "entity_id": "light.kitchen",
          "label": "",             // 0-48 characters; "" = use the entity's friendly name
          "x": 0, "y": 0,          // top-left cell, 0-based
          "w": 1, "h": 1,          // size in cells, >= 1, inside the grid
          "temperature_entity": "",  // climate only: "" or a sensor.* entity id
          "humidity_entity": ""      // climate only: "" or a sensor.* entity id
        }
      ]
    }
  ]
}
```

Ids (page and tile) are 1-32 characters from `A-Z a-z 0-9 _ -`.
`temperature_entity` and `humidity_entity` are always present in layouts the
integration sends. On `climate` tiles each is `""` (use the climate entity's own
reading) or a `sensor.*` entity id. On other kinds both must be `""`. The integration
rejects layouts that break any of these rules (see `invalid_layout` below), so
the firmware can size its buffers from these limits.

Allowed entity domains per kind:

| kind | domains |
|---|---|
| light | `light`, `switch` |
| gate | `cover`, `switch`, `button` |
| garage | `cover`, `switch`, `button` |
| valve | `valve`, `switch` |
| climate | `climate` |
| toggle | `automation`, `input_boolean`, `switch` |
| scene | `scene`, `script`, `button`, `input_button` |
| sensor | `sensor`, `binary_sensor` |
| blind | `cover` |
| lock | `lock` |
| alarm | `alarm_control_panel` |
| media | `media_player` |

Page icons. `icon` is `""` or exactly one of these names. The firmware has
these glyphs compiled in, and the integration rejects anything else with
`invalid_layout`. The editor's picker uses this order:

```
mdi:home mdi:sofa mdi:bed mdi:silverware-fork-knife mdi:coffee mdi:shower
mdi:bathtub mdi:toilet mdi:desk mdi:wardrobe-outline mdi:baby-face-outline
mdi:toy-brick-outline mdi:washing-machine mdi:stairs mdi:door mdi:home-floor-0
mdi:home-floor-1 mdi:home-floor-2 mdi:home-floor-b mdi:home-roof mdi:balcony
mdi:garage mdi:car mdi:gate mdi:tree mdi:flower mdi:sprout mdi:pool mdi:paw
mdi:dumbbell mdi:television mdi:music mdi:gamepad-variant mdi:lightbulb-group
mdi:thermostat mdi:air-conditioner mdi:sun-snowflake-variant mdi:fan mdi:fire
mdi:snowflake mdi:water mdi:power-plug mdi:weather-sunny mdi:robot
mdi:shield-home mdi:cctv
```

Layouts the integration sends always include `icon`. Layouts saved before page
icons existed load with `icon: ""`.

The screen's content area is 800×412 px; the remaining 68 px are the screen's
top bar with the page tabs (icon left of the title). The tab row is shown even
when there is only one page. The editor draws the grid with the 800×412 aspect
ratio.

## 3. Websocket commands used by the screen

Any authenticated user may call these. The screen's MAC (12 upper-case hex
digits, no separators) is its `screen_id`.

### `esp32_screen/register`

```json
{"type":"esp32_screen/register","screen_id":"A0B1C2D3E4F5","name":"ESP32 Screen",
 "model":"ESP32-8048S043","sw_version":"0.2.0"}
```
Result: `{"screen_id":"A0B1C2D3E4F5","name":"<name as stored in HA>",
"time_zone":"Europe/Lisbon","posix_tz":"WET0WEST,M3.5.0/1,M10.5.0"}`.
`time_zone` is Home Assistant's configured IANA zone. `posix_tz` is the same
zone as a POSIX TZ rule, taken from the footer of its TZif file, for the
screen's C library; it is `""` when it can't be determined.
Creates or updates the screen record and an HA device for it. A name already
stored in HA (renamed in the editor) wins over the name sent. `name`, `model`
and `sw_version` are optional (at most 64 characters each). A `screen_id` that is
not 12 upper-case hex digits fails with HA's `invalid_format` error.

### `esp32_screen/subscribe`

```json
{"type":"esp32_screen/subscribe","screen_id":"A0B1C2D3E4F5"}
```
Result: `null`, then events on the same id:

```jsonc
{"kind":"layout","layout":{ /* layout model; every tile's "label" is resolved
                               (never ""); unknown screens get {"version":1,"pages":[]} */ }}
{"kind":"states","tiles":{"t1":{ /* tile view model */ }, "t2":{...}}}
```

Order: `layout`, then `states` with every tile. After that, `states` carries only
tiles whose view model changed. Saving the layout in the editor sends `layout`
again followed by all `states`. While a subscription is open the screen counts as
online.

### Tile view model

All keys are always present, so the firmware can parse a fixed struct.

```jsonc
{
  "available": true,     // false: entity missing or unavailable
  "on": true,            // highlight the tile (light on, cover open, valve open, climate not off,
                         // toggle on/enabled, script running, binary_sensor on, lock not
                         // locked, alarm armed/triggered, media playing)
  "status": "Open",      // short state text: On/Off, Open/Closed/Opening/Closing/Stopped,
                         // Cooling/Heating/Off/Idle ..., Enabled/Disabled, Run/Running,
                         // "21.5 °C", Locked/Unlocked, Disarmed/Armed away, Playing ..., Unavailable
  "detail": "75%",       // secondary text or "": brightness, position,
                         // "22.5 °C → 24 °C · 45%", "Ran 5 min ago", "Artist – Title"
  "moving": false,       // cover/valve opening or closing, lock locking/unlocking/opening
  "brightness": 75,      // light 0-100; -1 when not dimmable or not a light
  "current": 22.5,       // climate current temperature; null otherwise
  "target": 24.0,        // climate target temperature; null otherwise
  "min": 16.0, "max": 30.0, "step": 0.5,   // climate limits; null otherwise
  "mode": "cool",        // climate hvac mode; "" otherwise
  "modes": ["off","cool","heat"],          // climate hvac modes; alarm arm modes; [] otherwise
  "unit": "°C",          // temperature unit; "" otherwise
  "humidity": 45,        // climate humidity in whole percent; null when unknown or not climate
  "position": 40,        // cover/valve position 0-100; -1 when not reported or not a cover/valve
  "volume": 30,          // media player volume 0-100; -1 when not settable or not a media player
  "code_required": false // lock/alarm takes a code (show a keypad); false otherwise
}
```

Details:

- `status` is `Unknown` when HA has no state yet (`unknown`), except for
  buttons, whose status is always `Press` (`Run` on scene tiles), and scene tiles.
- Climate temperatures are in HA's configured unit system (`unit`). `step` is the
  entity's `target_temp_step`, else 0.5 for °C and 1 for °F.
- Climate `current` is the numeric state of `temperature_entity` when it is set
  and available, rounded to 0.1. A sensor in another unit (°F, K) is converted to
  `unit`. Otherwise `current` is the climate's `current_temperature`, else null.
- Climate `humidity` is the numeric state of `humidity_entity` when it is set and
  available. Otherwise it is the climate's `current_humidity`, else null. It is
  rounded to a whole percent.
- The tile also updates when only one of its sensors changes.
- Climate `detail` is `"<current> → <target>"`, for example `"22 °C → 24 °C"`.
  Either side is left out when unknown. In a range mode (`heat_cool` with low/high
  targets) `target` is null and the target side is the range, for example
  `"23 °C → 21-24 °C"`. When `humidity` is known, `" · 45%"` is appended, or
  `detail` is just `"45%"` if there is nothing before it.
- A `switch` on a valve tile reports `Open`/`Closed`; on other tiles `On`/`Off`.
- Toggle tiles: an `automation` reports `Enabled`/`Disabled` with `detail` built
  from its `last_triggered`: `"Never ran"`, `"Ran just now"` (under a minute),
  `"Ran N min ago"`, `"Ran N h ago"` or `"Ran N d ago"`. The integration
  recomputes it every minute and sends `states` when the text changes.
  `input_boolean` and `switch` report `On`/`Off` with `detail` `""`. Other keys
  keep their defaults: `moving` false, `brightness` -1, climate keys null/""/[].
- Scene tiles: `status` is `Run`. A `script` that is running has `on` true and
  `status` `Running`. `detail` is `""`.
- Sensor tiles:
  - A `sensor` has its state plus unit as `status`, for example `"21.5 °C"` or
    `"830 W"`. A numeric value is rounded to the entity's display precision (the
    user's setting, else the integration's suggestion) when there is one;
    otherwise it is shown as HA reports it. A `timestamp` sensor shows local
    `HH:MM`, or `DD Mon HH:MM` when it is not today.
  - A `binary_sensor` uses wording by device class: door, garage_door, window,
    opening → `Open`/`Closed`; motion, occupancy, presence → `Detected`/`Clear`;
    moisture → `Wet`/`Dry`; anything else → `On`/`Off`. `on` = state on.
  - `detail` is `""`.
- Blind tiles (any cover) report like gate tiles, plus `position`.
  `position` is also filled on gate/garage/valve tiles when the entity reports one.
- Lock tiles: `status` is `Locked`, `Unlocked`, `Locking`, `Unlocking`, `Jammed`,
  `Open` or `Opening`. `on` = not `locked`. `code_required` = the lock has a
  `code_format`.
- Alarm tiles:
  - `status` is `Disarmed`, `Armed home`, `Armed away`, `Armed night`,
    `Armed vacation`, `Armed custom`, `Arming`, `Pending`, `Triggered` or
    `Disarming`.
  - `on` = armed or triggered.
  - `modes` = the panel's supported subset of `["arm_home","arm_away","arm_night","arm_vacation"]`,
    in that order.
  - `code_required` = the panel has a `code_format`, so it takes a code at all.
    It is the same flag for arming and disarming, because a panel whose arming
    needs no code (`code_arm_required` false) still needs one to disarm.
- Media tiles:
  - `status` is `Playing`, `Paused`, `Idle`, `Off`, `On`, `Standby` or
    `Buffering`.
  - `detail` is `"Artist – Title"`, else the title, else the source.
  - `volume` = `volume_level` × 100 when the player supports setting the
    volume.
  - `on` = playing.

### `esp32_screen/action`

```json
{"type":"esp32_screen/action","screen_id":"A0B1C2D3E4F5","tile_id":"t1","action":"toggle"}
{"type":"esp32_screen/action","screen_id":"A0B1C2D3E4F5","tile_id":"t1","action":"brightness","value":40}
```

| kind | actions |
|---|---|
| light | `toggle`, `brightness` (value 0-100; 0 turns off) |
| gate, garage | `toggle` (cover: stop if moving, else close if open, else open; switch: toggle; button: press), `open`, `close`, `stop` |
| valve | `toggle` (valve: as for covers), `open`, `close` |
| climate | `toggle` (off ⇄ last non-off mode, else first non-off mode), `set_temperature` (value number), `set_mode` (value string) |
| toggle | `toggle`, `on`, `off` (`<domain>.toggle` / `turn_on` / `turn_off` for automation, input_boolean, switch) |
| scene | `activate`, `toggle` (same as activate): `scene.turn_on`, `script.turn_on`, `button.press`, `input_button.press` |
| sensor | none (read-only; every action is `invalid_action`) |
| blind | `toggle`, `open`, `close`, `stop` (as for gate), `position` (value 0-100; `invalid_action` if the cover cannot be positioned) |
| lock | `lock`, `unlock`, `toggle` (unlock if locked, else lock); optional `value` = code string |
| alarm | `arm_home`, `arm_away`, `arm_night`, `arm_vacation` (only modes in `modes`), `disarm`; optional `value` = code string |
| media | `play_pause`, `next`, `previous`, `volume` (value 0-100), `toggle` (turn_on when off, else turn_off) |

An action not listed for the tile's kind is `invalid_action`.

How actions map to entities:

- `switch`: `open` turns it on, `close` turns it off; on a light tile `brightness`
  above 0 turns it on, 0 turns it off. `stop` is `invalid_action`.
- `button`: `toggle`, `open` and `close` all press it; `stop` is `invalid_action`.
- `cover`/`valve` that cannot stop: `toggle` while moving reverses direction, and
  `stop` is `invalid_action`.
- The "last non-off mode" is remembered per tile in HA's memory (lost on
  restart) and is also learned from state changes made elsewhere.
- `set_mode` with a mode not in `modes`, or a missing/non-numeric `value`, is
  `invalid_action`.
- A code `value` that is not a string is `invalid_action`. A wrong code is
  rejected by HA and comes back as `service_failed`.

Result: `null` on success. Errors use HA's standard error result with codes
`not_found` (screen or tile), `invalid_action`, or `service_failed`.

## 4. Websocket commands used by the editor panel (admin only)

| command | payload | result |
|---|---|---|
| `esp32_screen/screens` | — | `{"screens":[{"screen_id","name","model","sw_version","online","last_seen","layout","settings"}]}` (`last_seen` ISO 8601 or null) |
| `esp32_screen/layout/save` | `screen_id`, `layout` | `null`; error `invalid_layout` with a readable message |
| `esp32_screen/screen/update` | `screen_id`, optional `name`, optional `settings` (section 7) | `null`; `not_found` for unknown screens |
| `esp32_screen/screen/remove` | `screen_id` | `null`; `not_found` for unknown screens |
| `esp32_screen/subscribe_screens` | — | `null`, then an event `{"screens":[...]}` right away and again whenever any screen record or online state changes |

Non-admin users get HA's `unauthorized` error for all of these.
`layout/save` for a `screen_id` that has never registered creates its record
(and device), so a layout can be waiting before the screen first connects.
`screen/remove` also removes the HA device; a screen that is still subscribed
receives an empty layout, and reappears if it registers again. `last_seen` is
updated on register, subscribe, unsubscribe/disconnect and every action.

## 5. Firmware updates

Home Assistant offers new firmware to the screens. The screens download and
install it over Wi-Fi.

### Firmware folder

The integration reads `*.bin` ESP-IDF app images from
`<HA config>/esp32_screen/firmware/` and creates the folder on setup. It rescans
the folder on setup, every 60 s, and when the update entity is refreshed
(`homeassistant.update_entity`).

A file counts only if its app descriptor (`esp_app_desc_t`) checks out. All
offsets are from the start of the file:

| offset | field | rule |
|---|---|---|
| 32 | magic, u32 little-endian | must be `0xABCD5432` |
| 48 | `version`, 32 bytes, NUL-terminated | e.g. `0.3.0` |
| 80 | `project_name`, 32 bytes, NUL-terminated | must be `esp32-screen` |

Other files are ignored. The integration logs a warning once per file version.
The **latest** firmware is the valid file with the highest `version`, compared as
versions (so `0.10.0` > `0.9.1`). The integration computes its size and sha256.

### Update entity

Each screen's device has an update entity, "Firmware", titled "ESP32 Screen
firmware". `installed_version` is the `sw_version` from the last `register`,
`latest_version` is the latest file's version. With no valid file there is
nothing to install. Installing from HA works only while the screen has an open
`subscribe`; otherwise it fails with "Screen is offline".

### `ota` event

Installing sends this event on the screen's `esp32_screen/subscribe`
subscription:

```json
{"kind":"ota","version":"0.3.0","size":4950688,"sha256":"<64 hex>",
 "url":"/api/esp32_screen/firmware/<64 hex>.bin"}
```

### Download

`GET <HA base URL><url>` with `Authorization: Bearer <access token>`. The screen's
own non-admin user is enough. The response is the file as
`application/octet-stream` with `Content-Length`:

- `401` without a valid token.
- `404` when no current valid file has that sha256, for example because it was
  replaced meanwhile.

The screen writes the image to its inactive OTA partition and checks that the
size and sha256 match the event before it switches partitions. If the new
firmware fails to start, the bootloader rolls back to the previous one
automatically. The new firmware marks itself valid after running for 60 s;
a crash or reset before that rolls back. After a rollback, the restored
firmware reports it once, after subscribing, as an `ota_status` with state
`failed` and an error such as `"firmware 0.4.1 failed to start; rolled back to
0.4.0"` (no install is running at that point; Home Assistant logs it).

### `esp32_screen/ota_status`

The screen reports progress with:

```json
{"type":"esp32_screen/ota_status","screen_id":"A0B1C2D3E4F5",
 "state":"downloading","progress":42}
{"type":"esp32_screen/ota_status","screen_id":"A0B1C2D3E4F5",
 "state":"failed","progress":42,"error":"sha256 mismatch"}
```

- **Fields.** `state` is one of `downloading`, `verifying`, `restarting` or
  `failed`. `progress` is 0-100 and defaults to 0. `error` is optional text of
  up to 255 characters.
- **Result.** `null`. Any authenticated user may call it. An unknown screen
  gets `not_found`, a bad `state` gets `invalid_format`.
- **What HA does with it.**
  - While an install runs, the entity shows `in_progress` and
    `update_percentage` = `progress`.
  - `restarting` completes the install in HA.
  - `failed` makes the install fail with `error` as its message, and is logged
    as a warning.
  - With no report for 10 minutes, the install times out.
- **After the restart.** The screen registers again with its new `sw_version`,
  which updates `installed_version`.

## 6. Screen controls

The backlight and screensaver are HA entities on the screen's device:

| entity | what |
|---|---|
| `light.<screen>_backlight` | on/off + brightness (HA 1-255 ⇄ screen 1-100) |
| `select.<screen>_screensaver` | `off`, `clock`, `flip` |
| `number.<screen>_screensaver_delay` | idle minutes before the screensaver, 1-60 |
| `button.<screen>_wake` | wake the screen (leave the screensaver) |
| `button.<screen>_show_clock` | start the screensaver now |
| `binary_sensor.<screen>_screensaver` | on while the screensaver is showing |

These entities show what the screen last reported. They are unavailable until
the screen reports, and while it is offline. The buttons only need the screen
to be online. Changing an entity does not change its state in HA; the screen
applies the command and reports the new state.

### `command` event (HA → screen)

Sent on the screen's `esp32_screen/subscribe` subscription:

```jsonc
{"kind":"command","command":"brightness","value":50}           // 0-100, 0 = backlight off
{"kind":"command","command":"screensaver_mode","value":"flip"} // "off" | "clock" | "flip"
{"kind":"command","command":"screensaver_delay","value":600}   // seconds
{"kind":"command","command":"wake","value":null}
{"kind":"command","command":"sleep","value":null}              // show the screensaver now
```

`value` is always present (`null` for `wake`/`sleep`). Turning the backlight on
without a brightness sends the last brightness above 0 (100 if none is known).

### `esp32_screen/state` (screen → HA)

```json
{"type":"esp32_screen/state","screen_id":"A0B1C2D3E4F5","brightness":80,
 "screensaver_mode":"clock","screensaver_delay":300,"screensaver_active":false}
```

- **Fields.** All are required. `brightness` is 0-100,
  `screensaver_mode` is `off`, `clock` or `flip`, `screensaver_delay` is seconds
  (0-86400), and `screensaver_active` is a bool.
- **When to send.** Send it after every `subscribe`, and whenever any of these
  values changes, including changes made on the screen's own settings page.
- **Result.** `null`. An unknown screen gets `not_found`, an invalid value gets
  `invalid_format`.
- **Disconnect.** When the screen's last subscription closes, HA forgets the
  reported state.

## 7. Clock info and per-screen settings

Each screen record has settings, edited in the Screens panel. They are stored
with the screen, not in the layout:

| setting | values | default |
|---|---|---|
| `weather_entity` | `""` or a `weather.*` entity | `""` |
| `indoor_temperature_entity` | `""` or a `sensor.*` entity | `""` |
| `language` | `"en"`, `"pt"` | `"en"` |
| `clock_24h` | bool | `true` |

### Events on the screen's subscription

After `layout` and `states`, the subscription sends `settings`, then `info`:

```jsonc
{"kind":"settings","settings":{"language":"en","clock_24h":true}}
{"kind":"info","weather_condition":"sunny","weather_temperature":"22 °C",
 "indoor_temperature":"22.3 °C"}
```

- **Order.** Every `layout` is followed by `states`, `settings` and `info`.
  Changing the settings sends `settings` and `info` again without a new
  `layout`.
- **`weather_condition`.** The weather entity's state, which is HA's condition
  name (`sunny`, `cloudy`, `partlycloudy`, `rainy`, `pouring`,
  `lightning-rainy`, `snowy`, `fog`, `windy`, `clear-night`, `exceptional`
  and so on).
- **`weather_temperature`.** The weather entity's temperature rounded to a whole
  number, with its unit.
- **`indoor_temperature`.** The sensor's value in HA's temperature unit with one
  decimal, converted from °F/K if needed.
- **Empty values.** Each field is `""` when its entity is not configured,
  missing, unavailable or unknown.
- **When `info` is sent.** HA tracks the configured entities and sends `info`
  only when one of the three strings changes.

### Editor command

`esp32_screen/screen/update` (admin only) takes `settings`, a partial object of
the table above, as well as `name`. Both are optional. A value outside the
rules fails with `invalid_format`. The `screens` result includes each screen's
`settings`.

## 8. Notifications

HA actions (services):

- `esp32_screen.notify` with these fields:
  - `device_id`: one screen device or a list. Omit it to send to every online
    screen.
  - `title`: optional, up to 100 characters.
  - `message`: required, up to 500 characters.
  - `image_entity`: optional, a `camera.*` or `image.*` entity.
  - `timeout`: seconds, default 30. `0` keeps the notification until it is
    dismissed.

  It returns `{"id": "<id>", "screens": [<screen_id>...]}` when called with a
  response. Offline screens are skipped. If `device_id` is given and none of
  those screens is online, the action fails.
- `esp32_screen.dismiss` with optional `device_id` (same rules) and optional
  `id`.

### `notify` event

```json
{"kind":"notify","id":"5d38ba4f550bf018","title":"Front door",
 "message":"Someone is at the door",
 "image_url":"/api/esp32_screen/notification/5d38ba4f550bf018.jpg","timeout":30}
```

- **Fields.** All are always present. `id` is 16 lower-case hex digits, and the
  same for every screen of one call. `title` may be `""`. `image_url` is `""`
  without an image. `timeout` is seconds, and `0` means "until dismissed".
- **Several notifications.** A newer notification replaces the one on screen.

### Notification image

With `image_entity`, HA takes the entity's current picture (camera snapshot or
image entity) and converts it to a **baseline (non-progressive) JPEG** that
fits in 480×300. It keeps the aspect ratio, only scales down, uses quality 80,
and puts transparency on black. HA keeps it in memory for 10 minutes and serves
it at:

`GET <HA base URL>/api/esp32_screen/notification/<id>.jpg` with
`Authorization: Bearer <access token>`. The response is `image/jpeg`. Without a
valid token it is `401`; for an unknown or expired id, `404`.

### `dismiss` event

```json
{"kind":"dismiss","id":"5d38ba4f550bf018"}
{"kind":"dismiss","id":""}
```

The screen closes the notification with that `id`, ignoring others. An empty
`id` closes whatever notification is showing.
