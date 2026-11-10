//! URLs and signal strength.

/// Percent-encodes everything but RFC 3986 unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Normalizes what the user typed into a base URL: adds `http://` when no
/// scheme is given and drops trailing slashes.
pub fn normalize_url(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

/// `http://host:8123` → `ws://host:8123/api/websocket`.
pub fn websocket_url(base: &str) -> String {
    let rest = base
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| base.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| format!("ws://{base}"));
    format!("{rest}/api/websocket")
}

/// Maps RSSI to 1-4 signal bars.
pub fn wifi_bars(rssi: i8) -> i32 {
    match rssi {
        r if r >= -55 => 4,
        r if r >= -67 => 3,
        r if r >= -78 => 2,
        _ => 1,
    }
}

/// `AB:CD:…`: bytes as upper-case hex pairs joined by colons, as
/// `openssl x509 -noout -fingerprint -sha256` prints a fingerprint.
pub fn fingerprint_text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}
