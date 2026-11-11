// ESP32 Screen layout editor. Plain web component, no build step.
// Talks to the esp32_screen websocket commands documented in docs/protocol.md.

const KINDS = {
  light: { label: "Light", domains: ["light", "switch"], icon: "mdi:lightbulb-outline", iconOn: "mdi:lightbulb-on", color: "#f6b73c" },
  gate: { label: "Gate", domains: ["cover", "switch", "button"], icon: "mdi:gate", iconOn: "mdi:gate-open", color: "#4fc3f7" },
  garage: { label: "Garage", domains: ["cover", "switch", "button"], icon: "mdi:garage", iconOn: "mdi:garage-open", color: "#4fc3f7" },
  valve: { label: "Valve", domains: ["valve", "switch"], icon: "mdi:valve-closed", iconOn: "mdi:valve-open", color: "#4dd0e1" },
  climate: { label: "Climate", domains: ["climate"], icon: "mdi:thermostat", iconOn: "mdi:thermostat", color: "#ff8a65" },
  toggle: { label: "Toggle", domains: ["automation", "input_boolean", "switch"], icon: "mdi:toggle-switch-off-outline", iconOn: "mdi:toggle-switch", pickerIcon: "mdi:toggle-switch", color: "#81c784" },
  scene: { label: "Scene", domains: ["scene", "script", "button", "input_button"], icon: "mdi:play-circle-outline", iconOn: "mdi:play-circle", pickerIcon: "mdi:palette", color: "#ba68c8" },
  sensor: { label: "Sensor", domains: ["sensor", "binary_sensor"], icon: "mdi:gauge", iconOn: "mdi:gauge", color: "#90a4ae" },
  blind: { label: "Blind", domains: ["cover"], icon: "mdi:blinds", iconOn: "mdi:blinds-open", pickerIcon: "mdi:blinds-open", color: "#4fc3f7" },
  lock: { label: "Lock", domains: ["lock"], icon: "mdi:lock", iconOn: "mdi:lock-open-variant", pickerIcon: "mdi:lock", color: "#e57373" },
  alarm: { label: "Alarm", domains: ["alarm_control_panel"], icon: "mdi:shield-off-outline", iconOn: "mdi:shield-lock", pickerIcon: "mdi:shield-home", color: "#ef5350" },
  media: { label: "Media", domains: ["media_player"], icon: "mdi:speaker", iconOn: "mdi:speaker-play", pickerIcon: "mdi:music", color: "#4db6ac" },
};
// Page icons the firmware can draw (must match const.PAGE_ICONS), in picker order.
const PAGE_ICONS = [
  "mdi:home", "mdi:sofa", "mdi:bed", "mdi:silverware-fork-knife", "mdi:coffee", "mdi:shower", "mdi:bathtub",
  "mdi:toilet", "mdi:desk", "mdi:wardrobe-outline", "mdi:baby-face-outline", "mdi:toy-brick-outline",
  "mdi:washing-machine", "mdi:stairs", "mdi:door", "mdi:home-floor-0", "mdi:home-floor-1", "mdi:home-floor-2",
  "mdi:home-floor-b", "mdi:home-roof", "mdi:balcony", "mdi:garage", "mdi:car", "mdi:gate", "mdi:tree",
  "mdi:flower", "mdi:sprout", "mdi:pool", "mdi:paw", "mdi:dumbbell", "mdi:television", "mdi:music",
  "mdi:gamepad-variant", "mdi:lightbulb-group", "mdi:thermostat", "mdi:air-conditioner",
  "mdi:sun-snowflake-variant", "mdi:fan", "mdi:fire", "mdi:snowflake", "mdi:water", "mdi:power-plug",
  "mdi:weather-sunny", "mdi:robot", "mdi:shield-home", "mdi:cctv",
];

// Icons a light tile can show instead of the bulb (must match const.LIGHT_ICONS).
const LIGHT_ICONS = [
  "mdi:ceiling-light", "mdi:ceiling-light-multiple", "mdi:chandelier", "mdi:light-recessed",
  "mdi:track-light", "mdi:spotlight-beam", "mdi:lightbulb-spot", "mdi:dome-light", "mdi:vanity-light",
  "mdi:wall-sconce", "mdi:wall-sconce-flat", "mdi:wall-sconce-round", "mdi:coach-lamp",
  "mdi:coach-lamp-variant", "mdi:outdoor-lamp", "mdi:post-lamp", "mdi:bulkhead-light", "mdi:light-flood-down",
  "mdi:light-flood-up", "mdi:globe-light", "mdi:lamp", "mdi:lamps", "mdi:desk-lamp", "mdi:floor-lamp",
  "mdi:floor-lamp-torchiere", "mdi:lightbulb-fluorescent-tube", "mdi:led-strip-variant", "mdi:string-lights",
  "mdi:television-ambient-light", "mdi:ceiling-fan-light", "mdi:lightbulb-group",
];

// <ha-icon> when the frontend has it, else the icon name as text.
const iconHtml = (icon, cls = "") =>
  customElements.get("ha-icon")
    ? `<ha-icon class="${cls}" icon="${esc(icon)}"></ha-icon>`
    : `<span class="${cls} icon-text">${esc(icon.replace(/^mdi:/, ""))}</span>`;

const LIMITS = { pages: 6, cols: 8, rows: 6, title: 32, label: 48 };
const DRAG_THRESHOLD = 5;

const esc = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

const fmt = (n) => {
  if (n === null || n === undefined || Number.isNaN(Number(n))) return "";
  const r = Math.round(Number(n) * 10) / 10;
  return Number.isInteger(r) ? String(r) : r.toFixed(1);
};

const cap = (s) => (s ? s.charAt(0).toUpperCase() + s.slice(1).replace(/_/g, " ") : "");

const ago = (iso) => {
  if (!iso) return "never";
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} h ago`;
  return `${Math.floor(secs / 86400)} d ago`;
};

// Canonical key order so layouts can be compared as JSON.
const normLayout = (layout) => ({
  version: 1,
  pages: (layout?.pages || []).map((p) => ({
    id: p.id,
    title: p.title,
    icon: p.icon || "",
    cols: p.cols,
    rows: p.rows,
    tiles: (p.tiles || []).map((t) => ({
      id: t.id, kind: t.kind, entity_id: t.entity_id, label: t.label || "",
      x: t.x, y: t.y, w: t.w, h: t.h, icon: t.icon || "", tap: t.tap || "toggle", inline: t.inline || "",
      temperature_entity: t.temperature_entity || "", humidity_entity: t.humidity_entity || "",
    })),
  })),
});

const SENSOR_FIELDS = ["temperature_entity", "humidity_entity"];
// Entities the doorbell screen's button can open (doorbell.ACTION_SERVICES).
const DOORBELL_ACTION_DOMAINS = ["lock", "cover", "switch", "button", "input_button", "script"];

const ranAgo = (iso) => {
  if (!iso) return "Never ran";
  const secs = (Date.now() - new Date(iso).getTime()) / 1000;
  if (secs < 60) return "Ran just now";
  if (secs < 3600) return `Ran ${Math.floor(secs / 60)} min ago`;
  if (secs < 86400) return `Ran ${Math.floor(secs / 3600)} h ago`;
  return `Ran ${Math.floor(secs / 86400)} d ago`;
};

const toUnit = (value, from, to) => {
  if (!from || from === to) return value;
  const c = from === "°F" ? ((value - 32) * 5) / 9 : from === "K" ? value - 273.15 : from === "°C" ? value : null;
  if (c === null) return value;
  return to === "°F" ? (c * 9) / 5 + 32 : to === "K" ? c + 273.15 : c;
};

const BINARY_WORDS = {
  door: ["Open", "Closed"], garage_door: ["Open", "Closed"], window: ["Open", "Closed"], opening: ["Open", "Closed"],
  motion: ["Detected", "Clear"], occupancy: ["Detected", "Clear"], presence: ["Detected", "Clear"],
  moisture: ["Wet", "Dry"],
};

const rectsOverlap = (a, b) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

class Esp32ScreenPanel extends HTMLElement {
  constructor() {
    super();
    this.attachShadow({ mode: "open" });
    this._screens = [];
    this._local = []; // screens added by id, not yet known to HA
    this._sel = null;
    this._draft = null;
    this._base = "null";
    this._pageIdx = 0;
    this._selTile = null;
    this._newTile = null;
    this._error = null;
    this._notice = null;
    this._saving = false;
    this._drag = null;
    this._entityFilter = "";
    this._loaded = false;
    this._prevStates = null;

    const root = this.shadowRoot;
    root.addEventListener("click", (e) => this._onClick(e));
    root.addEventListener("input", (e) => this._onInput(e));
    root.addEventListener("change", (e) => this._onChange(e));
    root.addEventListener("pointerdown", (e) => this._onPointerDown(e));
    root.addEventListener("pointermove", (e) => this._onPointerMove(e));
    root.addEventListener("pointerup", (e) => this._onPointerUp(e));
    root.addEventListener("pointercancel", () => this._endDrag(false));
    root.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.target.dataset.enter) {
        e.preventDefault();
        this._act(e.target.dataset.enter, e.target);
      }
    });
    this._ticker = null;
  }

  // ---- HA properties ------------------------------------------------------

  set hass(hass) {
    const first = !this._hass;
    this._hass = hass;
    if (first) {
      this._render();
      this._subscribe();
    } else {
      this._updateLive();
    }
  }
  get hass() { return this._hass; }

  set narrow(value) {
    this._narrow = !!value;
    this.toggleAttribute("narrow", this._narrow);
    const btn = this.shadowRoot.querySelector(".menu");
    if (btn) btn.hidden = !this._narrow;
  }
  get narrow() { return this._narrow; }

  set panel(value) { this._panel = value; }
  get panel() { return this._panel; }

  connectedCallback() {
    if (this._hass && !this._unsubPromise) this._subscribe();
    this._ticker = setInterval(() => {
      this._renderScreens();
      if (!this._drag) this._renderGrid(); // relative "Ran 5 min ago" texts
    }, 30000);
  }

  disconnectedCallback() {
    clearInterval(this._ticker);
    if (this._unsubPromise) {
      this._unsubPromise.then((unsub) => unsub && unsub()).catch(() => {});
      this._unsubPromise = null;
    }
  }

  _subscribe() {
    if (!this._hass || this._unsubPromise || !this.isConnected) return;
    this._unsubPromise = this._hass.connection
      .subscribeMessage((msg) => this._onScreens(msg.screens || []), { type: "esp32_screen/subscribe_screens" })
      .catch((err) => {
        this._loaded = true;
        this._error = `Cannot load screens: ${err.message || err.code || err}. Is the ESP32 Screen integration set up?`;
        this._unsubPromise = null;
        this._render();
        return null;
      });
  }

  // ---- data -----------------------------------------------------------------

  get _dirty() {
    return this._draft !== null && JSON.stringify(normLayout(this._draft)) !== this._base;
  }

  _allScreens() {
    const known = new Set(this._screens.map((s) => s.screen_id));
    this._local = this._local.filter((s) => !known.has(s.screen_id));
    return [...this._screens, ...this._local];
  }

  _screen() {
    return this._allScreens().find((s) => s.screen_id === this._sel) || null;
  }

  _page() {
    return this._draft?.pages[this._pageIdx] || null;
  }

  _onScreens(screens) {
    const firstLoad = !this._loaded;
    this._loaded = true;
    this._screens = screens;
    const all = this._allScreens();
    if (!this._sel || !all.some((s) => s.screen_id === this._sel)) {
      this._loadScreen(all[0]?.screen_id || null);
      this._render();
      return;
    }
    const server = this._screen();
    const serverJson = JSON.stringify(normLayout(server.layout));
    if (!this._dirty && serverJson !== this._base) {
      this._loadScreen(this._sel, true);
      this._render();
      return;
    }
    if (firstLoad) {
      this._render();
      return;
    }
    this._renderScreens();
    this._renderHead();
    this._renderSettings();
  }

  _loadScreen(screenId, keepPage = false) {
    this._sel = screenId;
    const screen = this._screen();
    const layout = normLayout(screen?.layout);
    this._draft = screen ? layout : null;
    this._base = JSON.stringify(layout);
    if (!keepPage) this._pageIdx = 0;
    this._pageIdx = Math.min(this._pageIdx, Math.max(0, layout.pages.length - 1));
    this._selTile = null;
    this._newTile = null;
    this._iconPicker = false;
    if (!keepPage) this._error = null;
  }

  _nextId(prefix, used) {
    let n = 1;
    for (const id of used) {
      const m = new RegExp(`^${prefix}(\\d+)$`).exec(id);
      if (m) n = Math.max(n, Number(m[1]) + 1);
    }
    while (used.includes(`${prefix}${n}`)) n += 1;
    return `${prefix}${n}`;
  }

  _allTileIds() {
    return this._draft.pages.flatMap((p) => p.tiles.map((t) => t.id));
  }

  _fits(page, rect, ignoreId) {
    if (rect.x < 0 || rect.y < 0 || rect.w < 1 || rect.h < 1) return false;
    if (rect.x + rect.w > page.cols || rect.y + rect.h > page.rows) return false;
    return !page.tiles.some((t) => t.id !== ignoreId && rectsOverlap(t, rect));
  }

  _entities(kind) {
    const domains = KINDS[kind]?.domains || [];
    return Object.values(this._hass.states)
      .filter((s) => domains.includes(s.entity_id.split(".")[0]))
      .map((s) => ({ id: s.entity_id, name: s.attributes.friendly_name || s.entity_id, state: s.state }))
      .sort((a, b) => a.name.localeCompare(b.name));
  }

  // ---- live tile state (mirrors view_model.py closely enough for a preview) -------

  _tileView(tile) {
    const st = this._hass.states[tile.entity_id];
    const kind = KINDS[tile.kind] || KINDS.light;
    const name = tile.label || st?.attributes.friendly_name || tile.entity_id || "No entity";
    const view = { name, on: false, status: "Unavailable", detail: "", available: false, missing: !st };
    if (!tile.entity_id) return { ...view, status: "Pick an entity", missing: true };
    if (!st) return { ...view, status: "Not found" };
    if (st.state === "unavailable") return view;
    view.available = true;
    const domain = st.entity_id.split(".")[0];
    const a = st.attributes;
    if (tile.kind === "scene") {
      view.on = domain === "script" && st.state === "on";
      view.status = view.on ? "Running" : "Run";
    } else if (st.state === "unknown") {
      view.status = "Unknown";
    } else if (tile.kind === "sensor") {
      if (domain === "binary_sensor") {
        const words = BINARY_WORDS[a.device_class] || ["On", "Off"];
        view.on = st.state === "on";
        view.status = view.on ? words[0] : words[1];
      } else {
        let value = st.state;
        const precision = this._hass.entities?.[st.entity_id]?.display_precision;
        if (precision != null && value !== "" && Number.isFinite(Number(value))) value = Number(value).toFixed(precision);
        view.status = a.unit_of_measurement ? `${value} ${a.unit_of_measurement}` : value;
      }
    } else if (domain === "lock") {
      view.on = st.state !== "locked";
      view.status = cap(st.state);
      if (a.code_format) view.detail = "Code";
    } else if (domain === "alarm_control_panel") {
      view.on = st.state.startsWith("armed_") || st.state === "triggered";
      view.status = st.state === "armed_custom_bypass" ? "Armed custom" : cap(st.state);
    } else if (domain === "media_player") {
      view.on = st.state === "playing";
      view.status = cap(st.state);
      const artist = a.media_artist || a.media_album_artist;
      view.detail = artist && a.media_title ? `${artist} – ${a.media_title}` : a.media_title || a.source || "";
      if (a.volume_level != null && (a.supported_features & 4)) view.value = `${Math.round(a.volume_level * 100)}%`;
    } else if (domain === "automation") {
      view.on = st.state === "on";
      view.status = view.on ? "Enabled" : "Disabled";
      view.detail = ranAgo(a.last_triggered);
    } else if (domain === "input_boolean") {
      view.on = st.state === "on";
      view.status = view.on ? "On" : "Off";
    } else if (domain === "light" || domain === "switch") {
      view.on = st.state === "on";
      view.status = tile.kind === "valve" ? (view.on ? "Open" : "Closed") : view.on ? "On" : "Off";
      const dimmable = (a.supported_color_modes || []).some((m) => m !== "onoff" && m !== "unknown");
      if (domain === "light" && dimmable && view.on && a.brightness) {
        view.detail = `${Math.max(1, Math.round((a.brightness * 100) / 255))}%`;
      }
    } else if (domain === "button") {
      view.status = "Press";
    } else if (domain === "cover" || domain === "valve") {
      view.on = st.state !== "closed";
      view.status = cap(st.state);
      if (a.current_position !== undefined && a.current_position !== null) view.detail = `${a.current_position}%`;
    } else if (domain === "climate") {
      view.on = st.state !== "off";
      const unit = this._hass.config?.unit_system?.temperature || "";
      view.status = a.hvac_action && st.state !== "off" ? cap(a.hvac_action) : cap(st.state);
      const parts = [];
      const tSensor = this._sensorReading(tile.temperature_entity);
      const current = tSensor ? toUnit(tSensor.value, tSensor.unit, unit) : a.current_temperature;
      const hSensor = this._sensorReading(tile.humidity_entity);
      const humidity = hSensor ? hSensor.value : a.current_humidity;
      if (current != null) parts.push(`${fmt(current)} ${unit}`);
      if (a.temperature != null) parts.push(`${fmt(a.temperature)} ${unit}`);
      else if (a.target_temp_low != null && a.target_temp_high != null) {
        parts.push(`${fmt(a.target_temp_low)}-${fmt(a.target_temp_high)} ${unit}`);
      }
      view.detail = parts.join(" → ");
      if (humidity != null) view.detail = view.detail ? `${view.detail} · ${Math.round(humidity)}%` : `${Math.round(humidity)}%`;
      if (a.temperature != null) view.value = `${fmt(a.temperature)} ${unit}`;
      view.heat = st.state === "heat" || a.hvac_action === "heating";
      // As the screen lays it out (TileView in ui/dashboard.slint).
      const deg = (n) => {
        const r = Math.round(Number(n) * 10) / 10;
        return `${Number.isInteger(r) ? r : r.toFixed(1)}°`;
      };
      view.climate = {
        big: current != null ? deg(current) : a.temperature != null ? deg(a.temperature) : "",
        target: st.state !== "off" && current != null && a.temperature != null ? deg(a.temperature) : "",
        humidity: humidity != null ? `${Math.round(humidity)}%` : "",
      };
    } else {
      view.status = cap(st.state);
    }
    view.icon = tile.kind === "light" && tile.icon ? tile.icon : view.on ? kind.iconOn : kind.icon;
    return view;
  }

  _sensorReading(entityId) {
    const st = entityId ? this._hass.states[entityId] : null;
    if (!st || st.state === "unavailable" || st.state === "unknown") return null;
    const value = Number(st.state);
    if (st.state === "" || !Number.isFinite(value)) return null;
    return { value, unit: st.attributes.unit_of_measurement };
  }

  _updateLive() {
    if (this._drag || !this._draft) return;
    const page = this._page();
    if (!page) return;
    const prev = this._prevStates;
    const now = this._hass.states;
    this._prevStates = now;
    const ids = page.tiles.flatMap((t) => [t.entity_id, ...SENSOR_FIELDS.map((f) => t[f])]).filter(Boolean);
    if (prev && ids.every((id) => prev[id] === now[id])) return;
    this._renderGrid();
  }

  // ---- rendering ------------------------------------------------------------

  _render() {
    const root = this.shadowRoot;
    root.innerHTML = `
      <style>${STYLE}</style>
      <div class="wrap">
      <div class="header">
        <button class="icon-btn menu" data-action="menu" title="Menu" ${this._narrow ? "" : "hidden"}>
          <ha-icon icon="mdi:menu"></ha-icon>
        </button>
        <div class="title">Screens</div>
      </div>
      <div class="body">
        <aside class="screens" id="screens"></aside>
        <main class="main" id="main"></main>
      </div>
      </div>`;
    this._renderScreens();
    this._renderMain();
  }

  _renderScreens() {
    const el = this.shadowRoot.getElementById("screens");
    if (!el) return;
    const items = this._allScreens()
      .map((s) => {
        const pending = !this._screens.includes(s);
        const meta = pending ? "not saved yet" : s.online ? "online" : `last seen ${ago(s.last_seen)}`;
        return `
          <button class="screen-item ${s.screen_id === this._sel ? "selected" : ""}" data-action="select-screen" data-id="${esc(s.screen_id)}">
            <span class="dot ${s.online ? "online" : ""}"></span>
            <span class="screen-text">
              <span class="screen-name">${esc(s.name)}</span>
              <span class="screen-meta">${esc(s.screen_id)} · ${esc(meta)}</span>
            </span>
          </button>`;
      })
      .join("");
    el.innerHTML = `
      <div class="section-title">Screens</div>
      <div class="screen-list">${items || `<div class="muted pad">${this._loaded ? "No screens yet. A screen shows up here after it logs in, or add one by its id below." : "Loading..."}</div>`}</div>
      <div class="add-screen">
        <input id="new-screen-id" placeholder="Screen id (MAC)" maxlength="17" data-enter="add-screen" spellcheck="false">
        <button class="btn" data-action="add-screen">Add</button>
      </div>`;
  }

  _renderMain() {
    const el = this.shadowRoot.getElementById("main");
    if (!el) return;
    const screen = this._screen();
    if (!screen) {
      el.innerHTML = `
        <div id="error"></div>
        <div class="empty card">
          <ha-icon icon="mdi:tablet-dashboard"></ha-icon>
          <p>${this._loaded ? "Select or add a screen to design its layout." : "Loading screens..."}</p>
        </div>`;
      this._renderError();
      return;
    }
    el.innerHTML = `
      <div class="card head" id="head"></div>
      <div id="error"></div>
      <div class="card editor">
        <div class="tabs" id="tabs"></div>
        <div class="pagebar" id="pagebar"></div>
        <div class="editor-row">
          <div class="device-wrap" id="device-wrap"></div>
          <aside class="side" id="side"></aside>
        </div>
      </div>
      <div class="card settings" id="settings"></div>`;
    this._renderHead();
    this._renderSettings();
    this._renderError();
    this._renderTabs();
    this._renderPagebar();
    this._renderDevice();
    this._renderSide();
  }

  _renderSettings() {
    const el = this.shadowRoot.getElementById("settings");
    const screen = this._screen();
    if (!el || !screen) return;
    // Do not rebuild while the user is in one of the controls.
    if (el.contains(this.shadowRoot.activeElement)) return;
    const pending = !this._screens.includes(screen);
    const st = { weather_entity: "", indoor_temperature_entity: "", language: "en", clock_24h: true,
      doorbell_entity: "", doorbell_camera: "", doorbell_action: "", doorbell_timeout: 60, ...(screen.settings || {}) };
    const states = Object.values(this._hass.states);
    const label = (s) => {
      const unit = s.attributes.unit_of_measurement || s.attributes.temperature_unit || "";
      const value = s.entity_id.startsWith("weather.") ? `${s.state}${s.attributes.temperature != null ? `, ${s.attributes.temperature} ${unit}` : ""}` : `${s.state}${unit ? ` ${unit}` : ""}`;
      return `${s.attributes.friendly_name || s.entity_id} (${value})`;
    };
    const byName = (a, b) => (a.attributes.friendly_name || a.entity_id).localeCompare(b.attributes.friendly_name || b.entity_id);
    const options = (list, current) => {
      const missing = current && !this._hass.states[current] ? `<option value="${esc(current)}" selected>${esc(current)} (not found)</option>` : "";
      return missing + list.map((s) => `<option value="${esc(s.entity_id)}" ${s.entity_id === current ? "selected" : ""}>${esc(label(s))}</option>`).join("");
    };
    const weather = states.filter((s) => s.entity_id.startsWith("weather.")).sort(byName);
    const sensors = states.filter((s) => s.entity_id.startsWith("sensor.")).sort(byName);
    const isTemp = (s) => s.attributes.device_class === "temperature" || (!s.attributes.device_class && ["°C", "°F", "K"].includes(s.attributes.unit_of_measurement));
    const html = `
      <div class="settings-head">
        <div class="side-title">Screen settings</div>
        <span class="muted">${pending ? "Save a layout first; settings belong to a screen HA knows." : this._settingsNotice ? esc(this._settingsNotice) : "Applied right away."}</span>
      </div>
      <div class="settings-grid">
        <label class="field">Weather
          <select data-change="setting-weather_entity" ${pending ? "disabled" : ""}>
            <option value="" ${st.weather_entity ? "" : "selected"}>None</option>
            ${options(weather, st.weather_entity)}
          </select>
        </label>
        <label class="field">Indoor temperature
          <select data-change="setting-indoor_temperature_entity" ${pending ? "disabled" : ""}>
            <option value="" ${st.indoor_temperature_entity ? "" : "selected"}>None</option>
            <optgroup label="Temperature sensors">${options(sensors.filter(isTemp), st.indoor_temperature_entity)}</optgroup>
            <optgroup label="Other sensors">${options(sensors.filter((s) => !isTemp(s)), st.indoor_temperature_entity)}</optgroup>
          </select>
        </label>
        <label class="field">Language
          <select data-change="setting-language" ${pending ? "disabled" : ""}>
            <option value="en" ${st.language === "en" ? "selected" : ""}>English</option>
            <option value="pt" ${st.language === "pt" ? "selected" : ""}>Português</option>
          </select>
        </label>
        <label class="field check">
          <span>Clock</span>
          <span class="check-row"><input type="checkbox" data-change="setting-clock_24h" ${st.clock_24h ? "checked" : ""} ${pending ? "disabled" : ""}> 24-hour</span>
        </label>
      </div>
      <div class="settings-head">
        <div class="side-title">Doorbell</div>
        <span class="muted">When it rings, the screen wakes up and shows the camera live.</span>
      </div>
      <div class="settings-grid">
        <label class="field">Rings with
          <select data-change="setting-doorbell_entity" ${pending ? "disabled" : ""}>
            <option value="" ${st.doorbell_entity ? "" : "selected"}>No doorbell</option>
            <optgroup label="Events">${options(states.filter((s) => s.entity_id.startsWith("event.")).sort(byName), st.doorbell_entity)}</optgroup>
            <optgroup label="Binary sensors">${options(states.filter((s) => s.entity_id.startsWith("binary_sensor.")).sort(byName), st.doorbell_entity)}</optgroup>
          </select>
        </label>
        <label class="field">Camera
          <select data-change="setting-doorbell_camera" ${pending ? "disabled" : ""}>
            <option value="" ${st.doorbell_camera ? "" : "selected"}>No camera</option>
            ${options(states.filter((s) => s.entity_id.startsWith("camera.")).sort(byName), st.doorbell_camera)}
          </select>
        </label>
        <label class="field">Button on the screen
          <select data-change="setting-doorbell_action" ${pending ? "disabled" : ""}>
            <option value="" ${st.doorbell_action ? "" : "selected"}>None</option>
            ${options(states.filter((s) => DOORBELL_ACTION_DOMAINS.includes(s.entity_id.split(".")[0])).sort(byName), st.doorbell_action)}
          </select>
        </label>
        <label class="field">Show for
          <select data-change="setting-doorbell_timeout" ${pending ? "disabled" : ""}>
            ${[30, 60, 120, 300].map((s) => `<option value="${s}" ${Number(st.doorbell_timeout) === s ? "selected" : ""}>${s < 60 ? `${s} s` : `${s / 60} min`}</option>`).join("")}
          </select>
        </label>
      </div>`;
    if (el._html === html) return;
    el._html = html;
    el.innerHTML = html;
  }

  async _saveSetting(key, value) {
    const screen = this._screen();
    if (!screen) return;
    try {
      await this._hass.connection.sendMessagePromise({ type: "esp32_screen/screen/update", screen_id: screen.screen_id, settings: { [key]: value } });
      this._settingsNotice = "Saved";
      this._error = null;
    } catch (err) {
      this._error = err.message || String(err);
      this._settingsNotice = null;
    }
    this._renderError();
    this.shadowRoot.activeElement?.blur?.();
    this._renderSettings();
  }

  _renderHead() {
    const el = this.shadowRoot.getElementById("head");
    const screen = this._screen();
    if (!el || !screen) return;
    const pending = !this._screens.includes(screen);
    const dirty = this._dirty;
    const status = pending
      ? `<span class="chip">Waiting for first save</span>`
      : screen.online
        ? `<span class="chip online">Online</span>`
        : `<span class="chip">Offline · last seen ${esc(ago(screen.last_seen))}</span>`;
    const info = [screen.model, screen.sw_version ? `v${screen.sw_version}` : null].filter(Boolean).join(" · ");
    const nameInput = this.shadowRoot.getElementById("screen-name");
    const keepName = nameInput && this.shadowRoot.activeElement === nameInput ? nameInput.value : null;
    const html = `
      <div class="head-row">
        <div class="head-info">
          <div class="rename">
            <input id="screen-name" value="${esc(keepName ?? screen.name)}" maxlength="64" data-enter="rename" ${pending ? "disabled" : ""} aria-label="Screen name">
            <button class="btn" data-action="rename" ${pending ? "disabled" : ""}>Rename</button>
          </div>
          <div class="head-meta">
            ${status}
            <span class="muted mono" title="last seen ${esc(screen.last_seen || "never")}">${esc(screen.screen_id)}</span>
            ${info ? `<span class="muted">${esc(info)}</span>` : ""}
          </div>
        </div>
        <div class="head-actions">
          ${dirty ? `<span class="unsaved" title="You have unsaved changes"><span class="dot warn"></span>Unsaved changes</span>` : this._notice ? `<span class="muted">${esc(this._notice)}</span>` : ""}
          <button class="btn" data-action="discard" ${dirty ? "" : "disabled"}>Discard</button>
          <button class="btn primary" data-action="save" ${dirty && !this._saving ? "" : "disabled"}>${this._saving ? "Saving..." : "Save"}</button>
          <button class="icon-btn danger" data-action="remove-screen" title="Remove screen"><ha-icon icon="mdi:delete-outline"></ha-icon></button>
        </div>
      </div>`;
    // Unchanged? Keep the DOM, so a click in flight is not lost to a re-render.
    if (el._html === html) return;
    el._html = html;
    el.innerHTML = html;
    if (keepName !== null) {
      const input = this.shadowRoot.getElementById("screen-name");
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    }
  }

  _renderError() {
    const el = this.shadowRoot.getElementById("error");
    if (!el) return;
    el.innerHTML = this._error
      ? `<div class="error-box"><ha-icon icon="mdi:alert-circle-outline"></ha-icon><span>${esc(this._error)}</span>
         <button class="icon-btn" data-action="dismiss-error" title="Dismiss"><ha-icon icon="mdi:close"></ha-icon></button></div>`
      : "";
  }

  _renderTabs() {
    const el = this.shadowRoot.getElementById("tabs");
    if (!el) return;
    const pages = this._draft.pages;
    el.innerHTML = `
      ${pages.map((p, i) => `<button class="tab ${i === this._pageIdx ? "active" : ""}" data-action="select-page" data-index="${i}">${p.icon ? iconHtml(p.icon, "tab-icon") : ""}${esc(p.title || "(untitled)")}</button>`).join("")}
      <button class="tab add" data-action="add-page" ${pages.length >= LIMITS.pages ? "disabled" : ""} title="Add page"><ha-icon icon="mdi:plus"></ha-icon> Page</button>`;
  }

  _renderPagebar() {
    const el = this.shadowRoot.getElementById("pagebar");
    if (!el) return;
    const page = this._page();
    if (!page) {
      el.innerHTML = "";
      return;
    }
    const opts = (max, val) => Array.from({ length: max }, (_, i) => `<option value="${i + 1}" ${i + 1 === val ? "selected" : ""}>${i + 1}</option>`).join("");
    const last = this._draft.pages.length - 1;
    el.innerHTML = `
      <div class="field icon-field">Icon
        <button class="icon-pick" data-action="icon-picker" title="Page icon" aria-haspopup="true" aria-expanded="${this._iconPicker ? "true" : "false"}">
          ${page.icon ? `${iconHtml(page.icon)}<span class="icon-pick-name">${esc(page.icon.replace(/^mdi:/, ""))}</span>` : `<span class="muted">No icon</span>`}
          <ha-icon class="caret" icon="mdi:menu-down"></ha-icon>
        </button>
        ${this._iconPicker ? `
          <div class="icon-popover" role="listbox">
            <button class="icon-choice none ${page.icon ? "" : "active"}" data-action="set-icon" data-icon="" title="No icon">No icon</button>
            ${PAGE_ICONS.map((icon) => `<button class="icon-choice ${icon === page.icon ? "active" : ""}" data-action="set-icon" data-icon="${icon}" title="${icon}">${iconHtml(icon)}</button>`).join("")}
          </div>` : ""}
      </div>
      <label class="field grow">Title<input data-input="page-title" value="${esc(page.title)}" maxlength="${LIMITS.title}"></label>
      <label class="field">Columns<select data-change="page-cols">${opts(LIMITS.cols, page.cols)}</select></label>
      <label class="field">Rows<select data-change="page-rows">${opts(LIMITS.rows, page.rows)}</select></label>
      <div class="pagebar-actions">
        <button class="icon-btn" data-action="page-left" title="Move page left" ${this._pageIdx === 0 ? "disabled" : ""}><ha-icon icon="mdi:chevron-left"></ha-icon></button>
        <button class="icon-btn" data-action="page-right" title="Move page right" ${this._pageIdx === last ? "disabled" : ""}><ha-icon icon="mdi:chevron-right"></ha-icon></button>
        <button class="icon-btn danger" data-action="delete-page" title="Delete page"><ha-icon icon="mdi:delete-outline"></ha-icon></button>
      </div>`;
  }

  _renderDevice() {
    const el = this.shadowRoot.getElementById("device-wrap");
    if (!el) return;
    const page = this._page();
    if (!page) {
      el.innerHTML = `
        <div class="empty">
          <p>This screen has no pages yet.</p>
          <button class="btn primary" data-action="add-page">Add a page</button>
        </div>`;
      return;
    }
    el.innerHTML = `
      <div class="device">
        <div class="topbar" id="topbar"></div>
        <div class="grid" id="grid"></div>
      </div>
      <div class="hint muted">Click an empty cell to add a tile. Drag a tile to move it, drag its corner to resize it.</div>`;
    this._renderTopbar();
    this._renderGrid();
  }

  _renderTopbar() {
    const el = this.shadowRoot.getElementById("topbar");
    if (!el) return;
    el.innerHTML = this._draft.pages
      .map((p, i) => `<span class="dtab ${i === this._pageIdx ? "active" : ""}" data-action="select-page" data-index="${i}">${p.icon ? iconHtml(p.icon, "dtab-icon") : ""}<span>${esc(p.title)}</span></span>`)
      .join("");
  }

  _renderGrid() {
    const el = this.shadowRoot.getElementById("grid");
    const page = this._page();
    if (!el || !page) return;
    this._prevStates = this._hass.states;
    const pct = (v, n) => `${(v / n) * 100}%`;
    const pos = (r) => `left:${pct(r.x, page.cols)};top:${pct(r.y, page.rows)};width:${pct(r.w, page.cols)};height:${pct(r.h, page.rows)}`;
    const occupied = new Set();
    for (const t of page.tiles) for (let y = t.y; y < t.y + t.h; y++) for (let x = t.x; x < t.x + t.w; x++) occupied.add(`${x},${y}`);
    let cells = "";
    for (let y = 0; y < page.rows; y++) {
      for (let x = 0; x < page.cols; x++) {
        if (occupied.has(`${x},${y}`)) continue;
        const nt = this._newTile;
        const pending = nt && x >= nt.x && x < nt.x + nt.w && y >= nt.y && y < nt.y + nt.h;
        cells += `<div class="cell ${pending ? "pending" : ""}" style="${pos({ x, y, w: 1, h: 1 })}" data-action="add-cell" data-x="${x}" data-y="${y}" title="Add a tile here"><div class="cell-inner"><ha-icon icon="mdi:plus"></ha-icon></div></div>`;
      }
    }
    const tiles = page.tiles
      .map((t) => {
        const v = this._tileView(t);
        const kind = KINDS[t.kind] || KINDS.light;
        const color = t.kind === "climate" && !v.heat ? "#64b5f6" : kind.color;
        return `
          <div class="tile ${v.on ? "on" : ""} ${v.available ? "" : "unavailable"} ${v.missing ? "missing" : ""} ${t.id === this._selTile ? "selected" : ""}"
               style="${pos(t)};--tile-accent:${color}" data-tile-id="${esc(t.id)}">
            <div class="tile-inner">
              ${v.climate && v.available ? `
              <div class="tile-top"><ha-icon icon="${v.icon || kind.icon}"></ha-icon><span class="tile-name climate-name">${esc(v.name)}</span></div>
              <div class="climate-now"><span class="climate-big">${esc(v.climate.big)}</span>${v.climate.target ? `<span class="climate-target">→ ${esc(v.climate.target)}</span>` : ""}</div>
              <div class="tile-status climate-bottom"><span class="climate-status">${esc(v.status)}</span>${v.climate.humidity ? `<span class="climate-humidity"><ha-icon icon="mdi:water"></ha-icon>${esc(v.climate.humidity)}</span>` : ""}</div>` : `
              <div class="tile-top"><ha-icon icon="${v.icon || kind.icon}"></ha-icon><span class="tile-kind">${esc(kind.label)}</span></div>
              <div class="tile-name">${esc(v.name)}</div>
              ${v.value && t.h >= 2 ? `<div class="tile-value">${esc(v.value)}</div>` : ""}
              <div class="tile-status">${esc(v.status)}${v.detail ? ` · <span class="tile-detail">${esc(v.detail)}</span>` : ""}</div>`}
              ${t.kind === "light" && t.inline ? `<div class="tile-bars">${t.inline.includes("brightness") ? `<span class="bar bright"></span>` : ""}${t.inline.includes("color") ? `<span class="bar hue"></span>` : ""}</div>` : ""}
            </div>
            <div class="handle" data-handle="${esc(t.id)}" title="Drag to resize"></div>
          </div>`;
      })
      .join("");
    el.innerHTML = cells + tiles + `<div class="ghost" id="ghost" hidden></div>`;
  }

  _renderSide() {
    const el = this.shadowRoot.getElementById("side");
    if (!el) return;
    const page = this._page();
    if (!page) {
      el.innerHTML = "";
      return;
    }
    if (this._newTile) {
      const t = this._newTile;
      el.innerHTML = `
        <div class="side-title">New tile <span class="muted">at column ${t.x + 1}, row ${t.y + 1}</span></div>
        ${this._kindPicker(t.kind)}
        ${this._entityPicker(t.kind, t.entity_id)}
        ${this._sensorPickers(t)}
        ${this._tileIconPicker(t)}
        ${this._lightOptions(t)}
        <label class="field">Label <span class="muted">(empty = entity name)</span><input data-input="tile-label" value="${esc(t.label)}" maxlength="${LIMITS.label}"></label>
        <div class="row2">
          <label class="field">Width<input type="number" min="1" max="${page.cols}" data-change="tile-w" value="${t.w}"></label>
          <label class="field">Height<input type="number" min="1" max="${page.rows}" data-change="tile-h" value="${t.h}"></label>
        </div>
        <div class="side-actions">
          <button class="btn" data-action="cancel-new">Cancel</button>
          <button class="btn primary" data-action="confirm-new" ${t.entity_id ? "" : "disabled"}>Add tile</button>
        </div>`;
      return;
    }
    const tile = page.tiles.find((x) => x.id === this._selTile);
    if (tile) {
      const st = this._hass.states[tile.entity_id];
      el.innerHTML = `
        <div class="side-title">Tile <span class="muted mono">${esc(tile.id)}</span></div>
        ${this._kindPicker(tile.kind)}
        ${this._entityPicker(tile.kind, tile.entity_id)}
        ${this._sensorPickers(tile)}
        ${this._tileIconPicker(tile)}
        ${this._lightOptions(tile)}
        <label class="field">Label <span class="muted">(empty = ${esc(st?.attributes.friendly_name || "entity name")})</span><input data-input="tile-label" value="${esc(tile.label)}" maxlength="${LIMITS.label}"></label>
        <div class="row4">
          <label class="field">X<input type="number" min="1" max="${page.cols}" data-change="tile-x" value="${tile.x + 1}"></label>
          <label class="field">Y<input type="number" min="1" max="${page.rows}" data-change="tile-y" value="${tile.y + 1}"></label>
          <label class="field">W<input type="number" min="1" max="${page.cols}" data-change="tile-w" value="${tile.w}"></label>
          <label class="field">H<input type="number" min="1" max="${page.rows}" data-change="tile-h" value="${tile.h}"></label>
        </div>
        <div class="side-actions">
          <button class="btn" data-action="deselect">Done</button>
          <button class="btn danger" data-action="delete-tile"><ha-icon icon="mdi:delete-outline"></ha-icon> Delete</button>
        </div>`;
      return;
    }
    el.innerHTML = `
      <div class="side-title">Page "${esc(page.title)}"</div>
      <p class="muted">${page.tiles.length} tile${page.tiles.length === 1 ? "" : "s"} on a ${page.cols}×${page.rows} grid.</p>
      <p class="muted">Select a tile to edit or delete it. Click an empty cell to add one.</p>
      <p class="muted">Changes go to the screen when you press <b>Save</b>.</p>`;
  }

  _sensorPickers(tile) {
    if (tile.kind !== "climate") return "";
    const sensors = Object.values(this._hass.states).filter((s) => s.entity_id.startsWith("sensor."));
    const picker = (field, title, matches) => {
      const current = tile[field] || "";
      const label = (s) => {
        const unit = s.attributes.unit_of_measurement || "";
        const value = s.state === "unavailable" || s.state === "unknown" ? s.state : `${s.state}${unit ? ` ${unit}` : ""}`;
        return `${s.attributes.friendly_name || s.entity_id} (${value})`;
      };
      const byName = (a, b) => (a.attributes.friendly_name || a.entity_id).localeCompare(b.attributes.friendly_name || b.entity_id);
      const good = sensors.filter(matches).sort(byName);
      const rest = sensors.filter((s) => !matches(s)).sort(byName);
      const opt = (s) => `<option value="${esc(s.entity_id)}" ${s.entity_id === current ? "selected" : ""}>${esc(label(s))}</option>`;
      const missing = current && !this._hass.states[current] ? `<option value="${esc(current)}" selected>${esc(current)} (not found)</option>` : "";
      return `
        <label class="field">${esc(title)}
          <select data-change="${field}">
            <option value="" ${current ? "" : "selected"}>Use the AC's own reading</option>
            ${missing}
            ${good.length ? `<optgroup label="Matching sensors">${good.map(opt).join("")}</optgroup>` : ""}
            ${rest.length ? `<optgroup label="Other sensors">${rest.map(opt).join("")}</optgroup>` : ""}
          </select>
        </label>`;
    };
    const unitOf = (s) => s.attributes.unit_of_measurement;
    return `
      ${picker("temperature_entity", "Temperature sensor", (s) => s.attributes.device_class === "temperature" || (!s.attributes.device_class && ["°C", "°F", "K"].includes(unitOf(s))))}
      ${picker("humidity_entity", "Humidity sensor", (s) => s.attributes.device_class === "humidity" || (!s.attributes.device_class && unitOf(s) === "%"))}`;
  }

  _lightOptions(tile) {
    if (tile.kind !== "light") return "";
    const opt = (value, label, current) => `<option value="${value}" ${value === current ? "selected" : ""}>${esc(label)}</option>`;
    const domain = (tile.entity_id || "").split(".")[0];
    const note = domain === "switch" && tile.inline ? `<span class="muted">A switch has no brightness or colour: nothing will show.</span>` : "";
    return `
      <div class="row2">
        <label class="field">Tap
          <select data-change="tile-tap">
            ${opt("toggle", "Turns on/off", tile.tap || "toggle")}
            ${opt("controls", "Opens controls", tile.tap || "toggle")}
          </select>
        </label>
        <label class="field">On the tile
          <select data-change="tile-inline">
            ${opt("", "Nothing", tile.inline || "")}
            ${opt("brightness", "Brightness", tile.inline || "")}
            ${opt("color", "Colour", tile.inline || "")}
            ${opt("brightness_color", "Brightness + colour", tile.inline || "")}
          </select>
        </label>
      </div>
      ${note}
      <p class="muted small">A long press always opens the controls. Sliders need room: brightness and colour fit best on tiles 2 wide or 2 high.</p>`;
  }

  _tileIconPicker(tile) {
    if (tile.kind !== "light") return "";
    const current = tile.icon || "";
    const choice = (icon, title) =>
      `<button class="icon-choice ${icon === current ? "active" : ""}" data-action="set-tile-icon" data-icon="${icon}" title="${esc(title)}">${iconHtml(icon || KINDS.light.iconOn)}</button>`;
    return `
      <div class="field">Icon <span class="muted">(${esc(current ? current.replace(/^mdi:/, "") : "bulb")})</span>
        <div class="tile-icons">${choice("", "Bulb (default)")}${LIGHT_ICONS.map((i) => choice(i, i.replace(/^mdi:/, ""))).join("")}</div>
      </div>`;
  }

  _kindPicker(kind) {
    return `
      <div class="field">Kind
        <div class="kinds">
          ${Object.entries(KINDS)
            .map(([k, d]) => `<button class="kind ${k === kind ? "active" : ""}" data-action="set-kind" data-kind="${k}" title="${esc(d.label)}"><ha-icon icon="${d.pickerIcon || d.iconOn}"></ha-icon><span>${esc(d.label)}</span></button>`)
            .join("")}
        </div>
      </div>`;
  }

  _entityPicker(kind, entityId) {
    const current = entityId ? this._hass.states[entityId] : null;
    return `
      <div class="field">Entity <span class="muted">(${esc(KINDS[kind].domains.join(", "))})</span>
        <div class="entity-current ${entityId ? "" : "none"}">${entityId ? `${esc(current?.attributes.friendly_name || entityId)} <span class="muted mono">${esc(entityId)}</span>` : "No entity selected"}</div>
        <input type="search" data-input="entity-filter" placeholder="Search entities" value="${esc(this._entityFilter)}">
        <div class="entity-list" id="entity-list">${this._entityOptions(kind, entityId)}</div>
      </div>`;
  }

  _entityOptions(kind, entityId) {
    const q = this._entityFilter.trim().toLowerCase();
    const list = this._entities(kind).filter((e) => !q || e.name.toLowerCase().includes(q) || e.id.includes(q));
    if (!list.length) return `<div class="muted pad">No matching entities.</div>`;
    return list
      .map((e) => `<button class="entity ${e.id === entityId ? "active" : ""}" data-action="pick-entity" data-entity="${esc(e.id)}"><span>${esc(e.name)}</span><span class="muted mono">${esc(e.id)} · ${esc(e.state)}</span></button>`)
      .join("");
  }

  _changed({ side = false, grid = true, tabs = false } = {}) {
    this._notice = null;
    if (grid) this._renderGrid();
    if (tabs) {
      this._renderTabs();
      this._renderTopbar();
    }
    if (side) this._renderSide();
    this._renderHead();
  }

  // ---- events ---------------------------------------------------------------

  _onClick(e) {
    if (this._iconPicker && !e.target.closest(".icon-popover, .icon-pick")) {
      this._iconPicker = false;
      this._renderPagebar();
    }
    const target = e.target.closest("[data-action]");
    if (!target || target.disabled) return;
    this._act(target.dataset.action, target);
  }

  _act(action, target) {
    const page = this._page();
    switch (action) {
      case "icon-picker":
        this._iconPicker = !this._iconPicker;
        this._renderPagebar();
        break;
      case "set-icon":
        if (!page) break;
        page.icon = target.dataset.icon || "";
        this._iconPicker = false;
        this._renderPagebar();
        this._changed({ tabs: true, grid: false });
        break;
      case "menu":
        this.dispatchEvent(new CustomEvent("hass-toggle-menu", { bubbles: true, composed: true }));
        break;
      case "select-screen": {
        const id = target.dataset.id;
        if (id === this._sel) break;
        if (this._dirty && !confirm("Discard unsaved changes to this screen?")) break;
        this._notice = null;
        this._loadScreen(id);
        this._renderScreens();
        this._renderMain();
        break;
      }
      case "add-screen": {
        const input = this.shadowRoot.getElementById("new-screen-id");
        const id = (input.value || "").replace(/[^0-9a-fA-F]/g, "").toUpperCase();
        if (!/^[0-9A-F]{12}$/.test(id)) {
          this._error = "A screen id is the screen's MAC address: 12 hex digits, e.g. A0:B1:C2:D3:E4:F5.";
          this._renderMain();
          break;
        }
        if (!this._allScreens().some((s) => s.screen_id === id)) {
          this._local.push({ screen_id: id, name: `ESP32 Screen ${id.slice(-4)}`, online: false, last_seen: null, layout: { version: 1, pages: [] } });
        }
        if (this._dirty && id !== this._sel && !confirm("Discard unsaved changes to this screen?")) break;
        this._error = null;
        this._loadScreen(id);
        this._render();
        break;
      }
      case "rename":
        this._rename();
        break;
      case "remove-screen":
        this._removeScreen();
        break;
      case "save":
        this._save();
        break;
      case "discard":
        if (confirm("Discard all unsaved changes?")) {
          this._error = null;
          this._loadScreen(this._sel, true);
          this._renderMain();
        }
        break;
      case "dismiss-error":
        this._error = null;
        this._renderError();
        break;
      case "select-page":
        this._pageIdx = Number(target.dataset.index);
        this._iconPicker = false;
        this._selTile = null;
        this._newTile = null;
        this._renderMain();
        break;
      case "add-page": {
        if (this._draft.pages.length >= LIMITS.pages) break;
        const id = this._nextId("p", this._draft.pages.map((p) => p.id));
        this._draft.pages.push({ id, title: `Page ${this._draft.pages.length + 1}`, icon: "", cols: 4, rows: 3, tiles: [] });
        this._pageIdx = this._draft.pages.length - 1;
        this._selTile = null;
        this._newTile = null;
        this._renderMain();
        break;
      }
      case "page-left":
      case "page-right": {
        const to = this._pageIdx + (action === "page-left" ? -1 : 1);
        if (to < 0 || to >= this._draft.pages.length) break;
        const pages = this._draft.pages;
        [pages[this._pageIdx], pages[to]] = [pages[to], pages[this._pageIdx]];
        this._pageIdx = to;
        this._renderMain();
        break;
      }
      case "delete-page":
        if (!page) break;
        if (page.tiles.length && !confirm(`Delete page "${page.title}" and its ${page.tiles.length} tiles?`)) break;
        this._draft.pages.splice(this._pageIdx, 1);
        this._pageIdx = Math.max(0, this._pageIdx - 1);
        this._selTile = null;
        this._newTile = null;
        this._renderMain();
        break;
      case "add-cell": {
        const x = Number(target.dataset.x);
        const y = Number(target.dataset.y);
        const kind = this._newTile?.kind || "light";
        this._selTile = null;
        this._entityFilter = "";
        this._newTile = { x, y, w: 1, h: 1, kind, entity_id: "", label: "", icon: "", tap: "toggle", inline: "", temperature_entity: "", humidity_entity: "" };
        this._fitNewTile();
        this._renderGrid();
        this._renderSide();
        break;
      }
      case "cancel-new":
        this._newTile = null;
        this._renderGrid();
        this._renderSide();
        break;
      case "confirm-new":
        this._confirmNewTile();
        break;
      case "set-kind":
        this._setKind(target.dataset.kind);
        break;
      case "set-tile-icon": {
        const tile = this._currentTile();
        if (!tile) break;
        tile.icon = target.dataset.icon || "";
        this._changed({ side: true });
        break;
      }
      case "pick-entity":
        this._pickEntity(target.dataset.entity);
        break;
      case "deselect":
        this._selTile = null;
        this._renderGrid();
        this._renderSide();
        break;
      case "delete-tile":
        if (!page) break;
        page.tiles = page.tiles.filter((t) => t.id !== this._selTile);
        this._selTile = null;
        this._changed({ side: true });
        break;
      default:
        break;
    }
  }

  _fitNewTile() {
    const page = this._page();
    const t = this._newTile;
    if (t.kind === "climate" && t.w === 1 && t.h === 1 && this._fits(page, { ...t, w: 2 })) t.w = 2;
  }

  _currentTile() {
    if (this._newTile) return this._newTile;
    return this._page()?.tiles.find((t) => t.id === this._selTile) || null;
  }

  _setKind(kind) {
    const tile = this._currentTile();
    if (!tile || !KINDS[kind] || tile.kind === kind) return;
    tile.kind = kind;
    if (tile.entity_id && !KINDS[kind].domains.includes(tile.entity_id.split(".")[0])) tile.entity_id = "";
    if (kind !== "climate") for (const f of SENSOR_FIELDS) tile[f] = "";
    if (kind !== "light") Object.assign(tile, { icon: "", tap: "toggle", inline: "" });
    if (this._newTile) this._fitNewTile();
    this._entityFilter = "";
    this._changed({ side: true });
  }

  _pickEntity(entityId) {
    const tile = this._currentTile();
    if (!tile) return;
    tile.entity_id = entityId;
    this._changed({ side: true });
  }

  _confirmNewTile() {
    const page = this._page();
    const t = this._newTile;
    if (!page || !t || !t.entity_id) return;
    if (!this._fits(page, t)) {
      this._error = "The new tile does not fit there: it would overlap another tile or leave the grid.";
      this._renderError();
      return;
    }
    const id = this._nextId("t", this._allTileIds());
    page.tiles.push({
      id, kind: t.kind, entity_id: t.entity_id, label: t.label.trim(), x: t.x, y: t.y, w: t.w, h: t.h,
      icon: t.icon || "", tap: t.tap || "toggle", inline: t.inline || "", temperature_entity: t.temperature_entity || "", humidity_entity: t.humidity_entity || "",
    });
    this._newTile = null;
    this._selTile = id;
    this._error = null;
    this._renderError();
    this._changed({ side: true });
  }

  _onInput(e) {
    const key = e.target.dataset.input;
    if (!key) return;
    const value = e.target.value;
    if (key === "entity-filter") {
      this._entityFilter = value;
      const tile = this._currentTile();
      const list = this.shadowRoot.getElementById("entity-list");
      if (tile && list) list.innerHTML = this._entityOptions(tile.kind, tile.entity_id);
    } else if (key === "page-title") {
      const page = this._page();
      if (!page) return;
      page.title = value;
      this._changed({ tabs: true, grid: false });
    } else if (key === "tile-label") {
      const tile = this._currentTile();
      if (!tile) return;
      tile.label = value;
      this._changed({ grid: !this._newTile });
    }
  }

  _onChange(e) {
    const key = e.target.dataset.change;
    if (!key) return;
    if (key.startsWith("setting-")) {
      const name = key.slice(8);
      const value = name === "clock_24h" ? e.target.checked : name === "doorbell_timeout" ? Number(e.target.value) : e.target.value;
      this._saveSetting(name, value);
      return;
    }
    const page = this._page();
    if (!page) return;
    const n = Number(e.target.value);
    if (key === "page-cols" || key === "page-rows") {
      const next = { cols: page.cols, rows: page.rows, [key === "page-cols" ? "cols" : "rows"]: n };
      const outside = page.tiles.filter((t) => t.x + t.w > next.cols || t.y + t.h > next.rows);
      if (outside.length) {
        this._error = `Move or shrink ${outside.map((t) => this._tileView(t).name).join(", ")} first: ${outside.length === 1 ? "it does" : "they do"} not fit in ${next.cols}×${next.rows}.`;
        this._renderError();
        this._renderPagebar();
        return;
      }
      Object.assign(page, next);
      this._newTile = null;
      this._renderPagebar();
      this._changed({ side: true });
      return;
    }
    if (key === "tile-tap" || key === "tile-inline") {
      const tile = this._currentTile();
      if (!tile) return;
      tile[key.slice(5)] = e.target.value;
      this._changed({ side: true, grid: !this._newTile });
      return;
    }
    if (SENSOR_FIELDS.includes(key)) {
      const tile = this._currentTile();
      if (!tile) return;
      tile[key] = e.target.value;
      this._changed({ grid: !this._newTile });
      return;
    }
    const tile = this._currentTile();
    if (!tile || !Number.isFinite(n)) return;
    const prop = key.slice(5); // x, y, w, h
    const value = prop === "x" || prop === "y" ? n - 1 : n;
    const candidate = { x: tile.x, y: tile.y, w: tile.w, h: tile.h, [prop]: value };
    if (!this._fits(page, candidate, this._newTile ? null : tile.id)) {
      this._error = "That position or size would overlap another tile or leave the grid.";
      this._renderError();
      this._renderSide();
      return;
    }
    Object.assign(tile, candidate);
    this._error = null;
    this._renderError();
    this._changed({ side: true });
  }

  // ---- drag to move / resize ----------------------------------------------------

  _cellAt(e) {
    const grid = this.shadowRoot.getElementById("grid");
    const page = this._page();
    const r = grid.getBoundingClientRect();
    const col = Math.floor(((e.clientX - r.left) / r.width) * page.cols);
    const row = Math.floor(((e.clientY - r.top) / r.height) * page.rows);
    return { col: Math.min(page.cols - 1, Math.max(0, col)), row: Math.min(page.rows - 1, Math.max(0, row)) };
  }

  _onPointerDown(e) {
    if (e.button !== undefined && e.button !== 0) return;
    const handle = e.target.closest("[data-handle]");
    const tileEl = e.target.closest("[data-tile-id]");
    if (!tileEl) return;
    const page = this._page();
    const tile = page?.tiles.find((t) => t.id === tileEl.dataset.tileId);
    if (!tile) return;
    e.preventDefault();
    const cell = this._cellAt(e);
    this._drag = {
      mode: handle ? "resize" : "move",
      tile,
      el: tileEl,
      startX: e.clientX,
      startY: e.clientY,
      offX: cell.col - tile.x,
      offY: cell.row - tile.y,
      cand: { x: tile.x, y: tile.y, w: tile.w, h: tile.h },
      valid: true,
      moved: false,
      pointerId: e.pointerId,
    };
    try {
      this.shadowRoot.getElementById("grid").setPointerCapture(e.pointerId);
    } catch (_err) {
      /* ignore */
    }
  }

  _onPointerMove(e) {
    const d = this._drag;
    if (!d || e.pointerId !== d.pointerId) return;
    if (!d.moved && Math.hypot(e.clientX - d.startX, e.clientY - d.startY) < DRAG_THRESHOLD) return;
    if (!d.moved) {
      d.moved = true;
      d.el.classList.add("dragging");
    }
    const page = this._page();
    const { col, row } = this._cellAt(e);
    const t = d.tile;
    let cand;
    if (d.mode === "move") {
      cand = {
        x: Math.min(page.cols - t.w, Math.max(0, col - d.offX)),
        y: Math.min(page.rows - t.h, Math.max(0, row - d.offY)),
        w: t.w,
        h: t.h,
      };
    } else {
      cand = { x: t.x, y: t.y, w: Math.max(1, Math.min(page.cols - t.x, col - t.x + 1)), h: Math.max(1, Math.min(page.rows - t.y, row - t.y + 1)) };
    }
    d.cand = cand;
    d.valid = this._fits(page, cand, t.id);
    const ghost = this.shadowRoot.getElementById("ghost");
    if (ghost) {
      ghost.hidden = false;
      ghost.classList.toggle("invalid", !d.valid);
      ghost.style.left = `${(cand.x / page.cols) * 100}%`;
      ghost.style.top = `${(cand.y / page.rows) * 100}%`;
      ghost.style.width = `${(cand.w / page.cols) * 100}%`;
      ghost.style.height = `${(cand.h / page.rows) * 100}%`;
    }
  }

  _onPointerUp(e) {
    const d = this._drag;
    if (!d || e.pointerId !== d.pointerId) return;
    this._endDrag(true);
  }

  _endDrag(commit) {
    const d = this._drag;
    if (!d) return;
    this._drag = null;
    try {
      this.shadowRoot.getElementById("grid")?.releasePointerCapture(d.pointerId);
    } catch (_err) {
      /* ignore */
    }
    if (!commit) {
      this._renderGrid();
      return;
    }
    if (!d.moved) {
      // A click: select the tile.
      this._newTile = null;
      this._selTile = d.tile.id;
      this._entityFilter = "";
      this._renderGrid();
      this._renderSide();
      return;
    }
    const t = d.tile;
    const c = d.cand;
    if (d.valid && (c.x !== t.x || c.y !== t.y || c.w !== t.w || c.h !== t.h)) {
      Object.assign(t, c);
      this._selTile = t.id;
      this._newTile = null;
      this._changed({ side: true });
    } else {
      if (!d.valid) {
        this._notice = "Tiles cannot overlap.";
        this._renderHead();
      }
      this._renderGrid();
    }
  }

  // ---- server calls ---------------------------------------------------------------

  async _save() {
    if (!this._draft || this._saving) return;
    this._saving = true;
    this._renderHead();
    const layout = normLayout(this._draft);
    try {
      await this._hass.connection.sendMessagePromise({ type: "esp32_screen/layout/save", screen_id: this._sel, layout });
      this._base = JSON.stringify(layout);
      this._error = null;
      this._notice = "Saved";
    } catch (err) {
      this._error = err.message || err.code || String(err);
      this._notice = null;
    } finally {
      this._saving = false;
    }
    this._renderError();
    this._renderHead();
    this._renderScreens();
  }

  async _rename() {
    const input = this.shadowRoot.getElementById("screen-name");
    const name = (input?.value || "").trim();
    const screen = this._screen();
    if (!screen || !name || name === screen.name) return;
    try {
      await this._hass.connection.sendMessagePromise({ type: "esp32_screen/screen/update", screen_id: screen.screen_id, name });
      this._notice = "Renamed";
      this._error = null;
    } catch (err) {
      this._error = err.message || String(err);
    }
    input.blur();
    this._renderError();
    this._renderHead();
  }

  async _removeScreen() {
    const screen = this._screen();
    if (!screen) return;
    const pending = !this._screens.includes(screen);
    if (!confirm(`Remove "${screen.name}" and its layout? ${screen.online ? "It is online and will show an empty layout." : ""}`)) return;
    if (pending) {
      this._local = this._local.filter((s) => s !== screen);
      this._loadScreen(this._allScreens()[0]?.screen_id || null);
      this._render();
      return;
    }
    try {
      await this._hass.connection.sendMessagePromise({ type: "esp32_screen/screen/remove", screen_id: screen.screen_id });
    } catch (err) {
      this._error = err.message || String(err);
      this._renderError();
    }
  }
}

const STYLE = `
  :host {
    display: block;
    height: 100%;
    background: var(--primary-background-color);
    color: var(--primary-text-color);
    font-family: var(--paper-font-body1_-_font-family, var(--ha-font-family-body, Roboto, sans-serif));
    --radius: var(--ha-card-border-radius, 12px);
    --card-bg: var(--card-background-color, var(--ha-card-background, #fff));
    --line: var(--divider-color, rgba(127,127,127,.25));
    --muted: var(--secondary-text-color, #727272);
    --accent: var(--primary-color, #03a9f4);
    --danger: var(--error-color, #db4437);
    --warn: var(--warning-color, #ffa600);
    --ok: var(--success-color, #43a047);
  }
  * { box-sizing: border-box; }
  [hidden] { display: none !important; }
  .header {
    display: flex; align-items: center; gap: 8px; height: var(--header-height, 56px); padding: 0 16px;
    background: var(--app-header-background-color, var(--primary-color)); color: var(--app-header-text-color, #fff);
    border-bottom: var(--app-header-border-bottom, none);
  }
  .header .title { font-size: 20px; font-weight: 400; }
  .header .icon-btn { color: inherit; }
  .body { display: grid; grid-template-columns: 260px minmax(0, 1fr); gap: 16px; padding: 16px; align-items: start; }
  .screens { background: var(--card-bg); border-radius: var(--radius); border: 1px solid var(--line); padding: 8px; }
  .section-title { font-weight: 500; padding: 8px; color: var(--muted); text-transform: uppercase; font-size: 12px; letter-spacing: .05em; }
  .screen-list { display: flex; flex-direction: column; gap: 2px; }
  .screen-item {
    display: flex; align-items: center; gap: 10px; width: 100%; text-align: left; padding: 10px; border: 0; border-radius: 8px;
    background: transparent; color: inherit; cursor: pointer; font: inherit;
  }
  .screen-item:hover { background: rgba(127,127,127,.1); }
  .screen-item.selected { background: color-mix(in srgb, var(--accent) 18%, transparent); }
  .screen-text { display: flex; flex-direction: column; min-width: 0; }
  .screen-name { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .screen-meta { font-size: 12px; color: var(--muted); }
  .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--disabled-text-color, #9e9e9e); flex: none; display: inline-block; }
  .dot.online { background: var(--ok); box-shadow: 0 0 0 3px color-mix(in srgb, var(--ok) 25%, transparent); }
  .dot.warn { background: var(--warn); }
  .add-screen { display: flex; gap: 6px; padding: 8px; border-top: 1px solid var(--line); margin-top: 8px; }
  .add-screen input { flex: 1; min-width: 0; }
  .main { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
  .card { background: var(--card-bg); border-radius: var(--radius); border: 1px solid var(--line); padding: 16px; box-shadow: var(--ha-card-box-shadow, none); }
  .head-row { display: flex; flex-wrap: wrap; gap: 12px; align-items: center; justify-content: space-between; }
  .head-info { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
  .rename { display: flex; gap: 6px; }
  .rename input { font-size: 18px; font-weight: 500; width: 260px; max-width: 60vw; }
  .head-meta { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; font-size: 13px; }
  .head-actions { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .unsaved { display: inline-flex; gap: 6px; align-items: center; color: var(--warn); font-weight: 500; font-size: 14px; }
  .chip { font-size: 12px; padding: 2px 10px; border-radius: 12px; background: rgba(127,127,127,.15); }
  .chip.online { background: color-mix(in srgb, var(--ok) 20%, transparent); color: var(--ok); font-weight: 500; }
  .mono { font-family: var(--code-font-family, ui-monospace, Menlo, monospace); font-size: 12px; }
  .muted { color: var(--muted); }
  .pad { padding: 8px; font-size: 14px; }
  input, select {
    font: inherit; color: var(--primary-text-color); background: var(--input-fill-color, var(--secondary-background-color, rgba(127,127,127,.08)));
    border: 1px solid var(--line); border-radius: 6px; padding: 7px 9px; min-width: 0;
  }
  input:focus, select:focus { outline: 2px solid var(--accent); outline-offset: -1px; }
  input:disabled { opacity: .6; }
  .btn {
    font: inherit; font-size: 14px; font-weight: 500; padding: 7px 14px; border-radius: 18px; cursor: pointer;
    border: 1px solid var(--line); background: transparent; color: var(--accent); display: inline-flex; align-items: center; gap: 4px;
  }
  .btn:hover:not(:disabled) { background: color-mix(in srgb, var(--accent) 10%, transparent); }
  .btn.primary { background: var(--accent); color: var(--text-primary-color, #fff); border-color: var(--accent); }
  .btn.primary:hover:not(:disabled) { filter: brightness(1.08); background: var(--accent); }
  .btn.danger { color: var(--danger); }
  .btn:disabled { opacity: .45; cursor: default; }
  .btn ha-icon { --mdc-icon-size: 18px; }
  .icon-btn {
    border: 0; background: transparent; color: var(--muted); width: 36px; height: 36px; border-radius: 50%; cursor: pointer;
    display: inline-flex; align-items: center; justify-content: center; flex: none;
  }
  .icon-btn:hover:not(:disabled) { background: rgba(127,127,127,.15); }
  .icon-btn.danger { color: var(--danger); }
  .icon-btn:disabled { opacity: .35; cursor: default; }
  .error-box {
    display: flex; gap: 10px; align-items: center; padding: 10px 12px; border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 14%, var(--card-bg)); color: var(--primary-text-color); border: 1px solid var(--danger);
  }
  .error-box > ha-icon { color: var(--danger); flex: none; }
  .error-box span { flex: 1; }
  .editor { display: flex; flex-direction: column; gap: 12px; }
  .tabs { display: flex; gap: 4px; flex-wrap: wrap; border-bottom: 1px solid var(--line); padding-bottom: 8px; }
  .tab {
    font: inherit; font-size: 14px; border: 0; background: transparent; color: var(--muted); padding: 8px 14px; border-radius: 8px;
    cursor: pointer; display: inline-flex; align-items: center; gap: 4px;
  }
  .tab:hover:not(:disabled) { background: rgba(127,127,127,.12); }
  .tab.active { color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent); font-weight: 500; }
  .tab.add { color: var(--accent); }
  .tab:disabled { opacity: .4; cursor: default; }
  .tab ha-icon { --mdc-icon-size: 18px; }
  .pagebar { display: flex; gap: 10px; flex-wrap: wrap; align-items: flex-end; }
  .pagebar-actions { display: flex; gap: 2px; margin-left: auto; }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--muted); }
  .field input, .field select { font-size: 14px; }
  .field.grow { flex: 1; min-width: 160px; }
  .editor-row { display: grid; grid-template-columns: minmax(0, 1fr) 300px; gap: 16px; align-items: start; }
  .device-wrap { min-width: 0; }
  .device {
    position: relative; width: 100%; max-width: 960px; aspect-ratio: 800 / 480; background: #0e1014; border-radius: 10px;
    margin: 6px; width: calc(100% - 12px); box-shadow: 0 0 0 6px #2a2d33, 0 4px 16px 6px rgba(0,0,0,.25); display: flex; flex-direction: column; overflow: hidden;
    user-select: none; -webkit-user-select: none;
  }
  .topbar {
    height: calc(100% * 68 / 480); flex: none; display: flex; align-items: center; gap: 4px; padding: 0 10px;
    background: #16191f; border-bottom: 1px solid #23272f; overflow: hidden;
  }
  .dtab {
    color: #8b93a1; font-size: clamp(10px, 1.6vw, 15px); padding: 4px 10px; border-radius: 6px; cursor: pointer; white-space: nowrap;
    display: inline-flex; align-items: center; gap: 6px;
  }
  .dtab-icon { --mdc-icon-size: clamp(14px, 2vw, 20px); }
  .icon-text { font-size: 10px; opacity: .8; }
  .tab-icon { margin-right: 2px; }
  .icon-field { position: relative; }
  .icon-pick {
    font: inherit; font-size: 14px; display: inline-flex; align-items: center; gap: 6px; height: 36px; padding: 0 6px 0 10px;
    border: 1px solid var(--line); border-radius: 6px; color: var(--primary-text-color); cursor: pointer; min-width: 120px;
    background: var(--input-fill-color, var(--secondary-background-color, rgba(127,127,127,.08)));
  }
  .icon-pick ha-icon { --mdc-icon-size: 20px; }
  .icon-pick .caret { margin-left: auto; color: var(--muted); }
  .icon-pick-name { max-width: 110px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .icon-popover {
    position: absolute; top: 100%; left: 0; z-index: 20; margin-top: 4px; padding: 8px; width: 316px; max-width: 80vw;
    display: grid; grid-template-columns: repeat(auto-fill, minmax(40px, 1fr)); gap: 4px;
    background: var(--card-bg); border: 1px solid var(--line); border-radius: 10px; box-shadow: 0 6px 24px rgba(0,0,0,.25);
  }
  .icon-choice {
    height: 40px; border: 1px solid transparent; border-radius: 8px; background: transparent; color: var(--primary-text-color);
    cursor: pointer; display: flex; align-items: center; justify-content: center; font: inherit; font-size: 11px; overflow: hidden;
  }
  .icon-choice:hover { background: rgba(127,127,127,.15); }
  .icon-choice.active { border-color: var(--accent); color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .icon-choice.none { grid-column: 1 / -1; height: 32px; }
  .tile-bars { display: flex; flex-direction: column; gap: 3px; margin-top: 4px; }
  .tile-bars .bar { display: block; height: 6px; border-radius: 3px; }
  .tile-bars .bright { background: linear-gradient(90deg, var(--tile-accent) 60%, rgba(127,127,127,.3) 60%); }
  .tile-bars .hue { background: linear-gradient(90deg, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00); }
  .small { font-size: 12px; margin: 4px 0 0; }
  .tile-icons { display: grid; grid-template-columns: repeat(auto-fill, minmax(36px, 1fr)); gap: 4px; margin-top: 4px; }
  .tile-icons .icon-choice { height: 36px; }
  .dtab.active { color: #fff; background: #2b3140; }
  .grid { position: relative; flex: 1; min-height: 0; touch-action: none; }
  .cell, .tile, .ghost { position: absolute; padding: 4px; }
  .cell { cursor: pointer; }
  .cell-inner {
    width: 100%; height: 100%; border: 1px dashed #2f3540; border-radius: 10px; display: flex; align-items: center; justify-content: center;
    color: transparent; transition: background .15s, color .15s;
  }
  .cell:hover .cell-inner, .cell.pending .cell-inner { background: rgba(255,255,255,.05); color: #8b93a1; border-color: #4a5260; }
  .cell.pending .cell-inner { border: 2px solid var(--accent); color: var(--accent); }
  .tile { cursor: grab; touch-action: none; }
  .tile-inner {
    position: relative; width: 100%; height: 100%; border-radius: 12px; background: #23272f; color: #e8eaed; padding: 8px 10px;
    display: flex; flex-direction: column; gap: 2px; overflow: hidden; border: 2px solid transparent;
  }
  .tile.on .tile-inner { background: color-mix(in srgb, var(--tile-accent) 22%, #23272f); }
  .tile.on .tile-top ha-icon { color: var(--tile-accent); }
  .tile.unavailable .tile-inner { opacity: .55; }
  .tile.missing .tile-inner { border-color: #c0392b; border-style: dashed; }
  .tile.selected .tile-inner { border-color: var(--accent); box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 35%, transparent); }
  .tile.dragging { opacity: .45; }
  .tile-inner { container: tile / size; }
  .tile-top { display: flex; align-items: center; gap: 6px; color: #9aa3b2; flex: none; }
  .tile-top ha-icon { --mdc-icon-size: clamp(14px, 26cqh, 30px); color: #9aa3b2; }
  .tile-kind { font-size: 10px; text-transform: uppercase; letter-spacing: .06em; }
  .tile-name { font-size: clamp(10px, 16cqh, 18px); font-weight: 500; line-height: 1.2; overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; flex: none; }
  .tile-status { font-size: clamp(10px, 14cqh, 14px); color: #aab2bf; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: auto; flex: none; }
  .tile-detail { color: #e8eaed; }
  .climate-name { -webkit-line-clamp: 1; color: inherit; }
  .tile-top .climate-name { color: #e8ebf0; }
  .climate-now { display: flex; align-items: baseline; gap: 8px; margin: auto 0; min-width: 0; }
  .climate-big { font-size: clamp(18px, 30cqh, 44px); font-weight: 500; line-height: 1; white-space: nowrap; }
  .climate-target { font-size: clamp(10px, 14cqh, 16px); color: #aab2bf; white-space: nowrap; }
  .climate-bottom { display: flex; justify-content: space-between; gap: 6px; }
  .tile.on .climate-status { color: var(--tile-accent); font-weight: 600; }
  .climate-humidity { display: inline-flex; align-items: center; gap: 2px; }
  .climate-humidity ha-icon { --mdc-icon-size: 1em; color: #aab2bf; }
  .tile-value { font-size: clamp(16px, 22cqh, 44px); font-weight: 300; margin: auto 0; line-height: 1; }
  @container tile (max-height: 70px) {
    .tile-kind { display: none; }
    .tile-name { -webkit-line-clamp: 1; }
  }
  @container tile (max-width: 90px) {
    .tile-kind { display: none; }
  }
  .handle {
    position: absolute; right: 4px; bottom: 4px; width: 22px; height: 22px; cursor: nwse-resize; touch-action: none;
    border-bottom-right-radius: 12px; background: linear-gradient(135deg, transparent 50%, rgba(255,255,255,.35) 50%);
    opacity: 0; transition: opacity .15s;
  }
  .tile:hover .handle, .tile.selected .handle { opacity: 1; }
  @media (hover: none) { .handle { opacity: .8; } }
  .ghost { pointer-events: none; z-index: 5; }
  .ghost::after {
    content: ""; display: block; width: 100%; height: 100%; border-radius: 12px; border: 2px dashed var(--accent);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
  }
  .ghost.invalid::after { border-color: #e53935; background: rgba(229,57,53,.2); }
  .hint { font-size: 13px; margin-top: 8px; }
  #error:empty { display: none; }
  .settings { display: flex; flex-direction: column; gap: 12px; }
  .settings-head { display: flex; gap: 12px; align-items: baseline; flex-wrap: wrap; }
  .settings-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 12px; align-items: end; }
  .field.check .check-row { display: flex; align-items: center; gap: 6px; height: 36px; font-size: 14px; color: var(--primary-text-color); }
  .field.check input { width: 18px; height: 18px; }
  .side p { margin: 0; line-height: 1.4; }
  .side { display: flex; flex-direction: column; gap: 12px; border-left: 1px solid var(--line); padding-left: 16px; min-width: 0; }
  .side-title { font-weight: 500; font-size: 16px; }
  .side-actions { display: flex; justify-content: space-between; gap: 8px; }
  .row2 { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
  .row4 { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
  .kinds { display: grid; grid-template-columns: repeat(4, 1fr); gap: 4px; }
  .kind {
    font: inherit; font-size: 11px; display: flex; flex-direction: column; align-items: center; gap: 2px; padding: 6px 2px; border-radius: 8px;
    border: 1px solid var(--line); background: transparent; color: var(--primary-text-color); cursor: pointer;
  }
  .kind.active { border-color: var(--accent); color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
  .entity-current { font-size: 14px; color: var(--primary-text-color); padding: 2px 0; }
  .entity-current.none { color: var(--danger); }
  .entity-list { max-height: 220px; overflow: auto; border: 1px solid var(--line); border-radius: 6px; }
  .entity {
    font: inherit; display: flex; flex-direction: column; align-items: flex-start; width: 100%; text-align: left; border: 0; border-bottom: 1px solid var(--line);
    background: transparent; color: var(--primary-text-color); padding: 6px 8px; cursor: pointer; font-size: 14px;
  }
  .entity:last-child { border-bottom: 0; }
  .entity:hover { background: rgba(127,127,127,.1); }
  .entity.active { background: color-mix(in srgb, var(--accent) 16%, transparent); }
  .empty { display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 40px 16px; color: var(--muted); text-align: center; }
  .empty ha-icon { --mdc-icon-size: 48px; }
  .wrap { container: page / inline-size; min-height: 100%; }
  .editor { container: editor / inline-size; }
  @container editor (max-width: 820px) {
    .editor-row { grid-template-columns: minmax(0, 1fr); }
    .side { border-left: 0; border-top: 1px solid var(--line); padding-left: 0; padding-top: 12px; }
  }
  @container page (max-width: 760px) {
    .body { grid-template-columns: minmax(0, 1fr); padding: 8px; gap: 8px; }
    .card { padding: 12px; }
    .screen-list { flex-direction: row; overflow-x: auto; }
    .screen-item { width: auto; flex: none; }
    .rename input { width: 180px; }
    .device { margin: 3px; width: calc(100% - 6px); box-shadow: 0 0 0 3px #2a2d33; }
  }
  :host([narrow]) .body { grid-template-columns: minmax(0, 1fr); padding: 8px; gap: 8px; }
  :host([narrow]) .editor-row { grid-template-columns: minmax(0, 1fr); }
  :host([narrow]) .screen-list { flex-direction: row; overflow-x: auto; }
  :host([narrow]) .screen-item { width: auto; flex: none; }
  :host([narrow]) .side { border-left: 0; border-top: 1px solid var(--line); padding-left: 0; padding-top: 12px; }
`;

if (!customElements.get("esp32-screen-panel")) {
  customElements.define("esp32-screen-panel", Esp32ScreenPanel);
}
