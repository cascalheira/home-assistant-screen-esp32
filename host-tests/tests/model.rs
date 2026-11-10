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
