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

/// Removes and returns the first whole JPEG (SOI ... EOI) from `buf`,
/// dropping whatever comes before it (multipart headers). Inside a JPEG's
/// data 0xFF is always followed by 0x00 or a restart marker, so the first
/// FF D9 after FF D8 ends it.
pub fn take_jpeg(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    let start = buf.windows(2).position(|w| w == [0xFF, 0xD8])?;
    let end = buf[start + 2..]
        .windows(2)
        .position(|w| w == [0xFF, 0xD9])?
        + start
        + 4;
    let frame = buf[start..end].to_vec();
    buf.drain(..end);
    Some(frame)
}
