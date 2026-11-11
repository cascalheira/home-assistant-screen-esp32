//! Parsing what the integration sends (docs/protocol.md).

use esp32_screen_host_tests::model::{Layout, TileState};

#[test]
fn layout_with_missing_and_extra_fields() {
    let layout: Layout = serde_json::from_str(
        r#"{"pages":[{"id":"p1","title":"Living room","icon":"mdi:sofa","cols":4,"rows":3,
            "tiles":[{"id":"t1","kind":"light","entity_id":"light.porch","label":"Porch",
                      "x":0,"y":0,"w":1,"h":1,"something_new":true}]},
            {"title":"Empty"}],"version":7}"#,
    )
    .unwrap();
    assert_eq!(layout.pages.len(), 2);
    assert_eq!(layout.pages[0].tiles[0].entity_id, "light.porch");
    assert_eq!(layout.pages[1].cols, 0);
    assert!(layout.pages[1].tiles.is_empty());
}

#[test]
fn tile_state_defaults_mean_unknown() {
    let state: TileState = serde_json::from_str("{}").unwrap();
    assert!(!state.available);
    assert_eq!(state.brightness, -1);
    assert_eq!(state.position, -1);
    assert_eq!(state.volume, -1);
    assert_eq!(state.target, None);
}

#[test]
fn climate_state() {
    let state: TileState = serde_json::from_str(
        r#"{"available":true,"on":true,"status":"Cooling","current":23.4,"target":21,
            "min":16,"max":30,"step":0.5,"humidity":47,"mode":"cool",
            "modes":["off","cool","heat"],"unit":"°C"}"#,
    )
    .unwrap();
    assert_eq!(state.target, Some(21.0));
    assert_eq!(state.step, Some(0.5));
    assert_eq!(state.humidity, Some(47.0));
    assert_eq!(state.modes, ["off", "cool", "heat"]);
}

#[test]
fn null_numbers_are_none() {
    let state: TileState = serde_json::from_str(r#"{"current":null,"position":40}"#).unwrap();
    assert_eq!(state.current, None);
    assert_eq!(state.position, 40);
}

#[test]
fn light_options() {
    let layout: Layout = serde_json::from_str(
        r#"{"pages":[{"tiles":[
            {"id":"a","kind":"light","tap":"controls","inline":"brightness_color"},
            {"id":"b","kind":"light"},
            {"id":"c","kind":"gate","tap":"controls","inline":"brightness"}]}]}"#,
    )
    .unwrap();
    let [a, b, c] = &layout.pages[0].tiles[..] else {
        panic!("three tiles")
    };
    assert!(a.tap_opens_controls() && a.inline_brightness() && a.inline_color());
    assert!(!b.tap_opens_controls() && !b.inline_brightness() && !b.inline_color());
    // Only light tiles have these options.
    assert!(!c.tap_opens_controls() && !c.inline_brightness());
}

#[test]
fn light_colour_state() {
    let state: TileState = serde_json::from_str(
        r##"{"on":true,"brightness":60,"hue":240,"rgb":"#0000ff",
             "kelvin":3000,"kelvin_min":2000,"kelvin_max":6500}"##,
    )
    .unwrap();
    assert!(state.has_light_controls());
    assert_eq!(state.color(), Some((0, 0, 255)));
    assert_eq!(state.kelvin_range(), Some((2000, 6500)));

    let plain: TileState = serde_json::from_str(r#"{"on":true,"rgb":"blue"}"#).unwrap();
    assert!(!plain.has_light_controls());
    assert_eq!((plain.hue, plain.kelvin), (-1, -1));
    assert_eq!(plain.color(), None);
    assert_eq!(plain.kelvin_range(), None);
}

#[test]
fn forecast_event() {
    use esp32_screen_host_tests::model::Forecast;
    let forecast: Forecast = serde_json::from_str(
        r#"{"kind":"forecast","unit":"°C","days":[
            {"date":"2026-11-12","weekday":4,"condition":"rainy","high":18,"low":11,"precipitation":80},
            {"date":"2026-11-13","weekday":5,"condition":"sunny","high":21,"low":null,"precipitation":null}]}"#,
    )
    .unwrap();
    assert_eq!(forecast.unit, "°C");
    assert_eq!(forecast.days.len(), 2);
    assert_eq!(forecast.days[0].precipitation, Some(80));
    assert_eq!(forecast.days[1].low, None);
    let empty: Forecast = serde_json::from_str(r#"{"unit":"","days":[]}"#).unwrap();
    assert!(empty.days.is_empty());
}
