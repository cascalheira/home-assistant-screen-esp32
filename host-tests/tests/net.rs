use esp32_screen_host_tests::logic::net::{normalize_url, url_encode, websocket_url, wifi_bars};

#[test]
fn normalize_adds_scheme_and_trims() {
    assert_eq!(
        normalize_url("homeassistant.local:8123"),
        "http://homeassistant.local:8123"
    );
    assert_eq!(normalize_url("  http://ha:8123/ "), "http://ha:8123");
    assert_eq!(
        normalize_url("https://ha.example.com//"),
        "https://ha.example.com"
    );
}

#[test]
fn websocket_url_follows_scheme() {
    assert_eq!(
        websocket_url("http://ha:8123"),
        "ws://ha:8123/api/websocket"
    );
    assert_eq!(
        websocket_url("https://ha.example.com"),
        "wss://ha.example.com/api/websocket"
    );
    assert_eq!(websocket_url("ha:8123"), "ws://ha:8123/api/websocket");
}

#[test]
fn url_encode_keeps_unreserved_only() {
    assert_eq!(url_encode("aZ09-_.~"), "aZ09-_.~");
    assert_eq!(url_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
}

#[test]
fn bars_thresholds() {
    assert_eq!(wifi_bars(-40), 4);
    assert_eq!(wifi_bars(-55), 4);
    assert_eq!(wifi_bars(-56), 3);
    assert_eq!(wifi_bars(-67), 3);
    assert_eq!(wifi_bars(-78), 2);
    assert_eq!(wifi_bars(-79), 1);
    assert_eq!(wifi_bars(i8::MIN), 1);
}
