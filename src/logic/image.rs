//! ESP-IDF app image headers (update files from Home Assistant or the SD
//! card).

/// Offset of `esp_app_desc_t` in an app image: after the 24-byte image
/// header and the first 8-byte segment header.
const DESC: usize = 32;
/// `esp_app_desc_t.magic_word`.
const MAGIC: u32 = 0xABCD_5432;

/// (project_name, version) from an app image's first bytes (>= 112).
pub fn descriptor(header: &[u8]) -> Option<(String, String)> {
    if header.len() < 112 {
        return None;
    }
    let magic = u32::from_le_bytes(header[DESC..DESC + 4].try_into().ok()?);
    if magic != MAGIC {
        return None;
    }
    let text = |range: core::ops::Range<usize>| {
        let raw = &header[range];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).into_owned()
    };
    Some((text(80..112), text(48..80)))
}
