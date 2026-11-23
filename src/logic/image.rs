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

/// The EXIF orientation of a JPEG (1-8; 1 = upright, also when unknown).
/// 3: upside down, 6: turn clockwise to view, 8: turn anticlockwise.
pub fn jpeg_orientation(data: &[u8]) -> u8 {
    let mut i = 2;
    if data.get(..2) != Some(&[0xFF, 0xD8][..]) {
        return 1;
    }
    // Walk the segments up to the image data.
    while i + 4 <= data.len() && data[i] == 0xFF {
        let marker = data[i + 1];
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if marker == 0xDA || len < 2 {
            break;
        }
        let body = data.get(i + 4..i + 2 + len).unwrap_or(&[]);
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return exif_orientation(&body[6..]).unwrap_or(1);
        }
        i += 2 + len;
    }
    1
}

fn exif_orientation(tiff: &[u8]) -> Option<u8> {
    let le = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    (0..count.min(64))
        .map(|n| ifd + 2 + n * 12)
        .find(|&entry| u16_at(entry) == Some(0x0112))
        .and_then(|entry| u16_at(entry + 8))
        .map(|v| v as u8)
        .filter(|v| (1..=8).contains(v))
}

/// How much to reduce a `width` x `height` JPEG while decoding (0-3, for
/// 1/1 to 1/8): as much as possible while one side still fills the
/// `target` box, and to at most `max_pixels`. None: too large even at 1/8.
pub fn jpeg_scale(width: u32, height: u32, target: (u32, u32), max_pixels: u64) -> Option<u8> {
    let fits = |s: u8| ((width >> s) as u64) * ((height >> s) as u64) <= max_pixels;
    let fills = |s: u8| (width >> s) >= target.0 || (height >> s) >= target.1;
    let mut best = None;
    for s in 0..=3u8 {
        if fits(s) && (fills(s) || best.is_none()) {
            best = Some(s);
        }
        if !fills(s) {
            break;
        }
    }
    best.or_else(|| fits(3).then_some(3))
}

/// The photo size to ask for: the screen's, or smaller (same shape, width a
/// multiple of 16) to fit `budget` pixels. None below a third of the width.
pub fn photo_box(screen: (u32, u32), budget: u64) -> Option<(u32, u32)> {
    let (w, h) = screen;
    if (w as u64) * (h as u64) <= budget {
        return Some(screen);
    }
    let s = (budget as f64 / (w as f64 * h as f64)).sqrt();
    let bw = ((w as f64 * s) as u32) / 16 * 16;
    let bh = (bw as u64 * h as u64 / w as u64) as u32;
    (bw >= w / 3).then_some((bw, bh))
}

/// Whether a `w` x `h` picture should fill the screen (cropping a little)
/// rather than show whole: its shape is within a third of the screen's
/// (a 4:3 photo fills a 16:9 screen; a portrait one shows whole).
pub fn photo_fills(w: u32, h: u32, screen: (u32, u32)) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    let r = (w as f64 / h as f64) / (screen.0 as f64 / screen.1 as f64);
    (0.75..=1.34).contains(&r)
}

/// Turns an RGB888 picture upright for EXIF `orientation` (1, 3, 6 or 8;
/// mirrored ones are shown unmirrored). Returns the pixels and the new size.
pub fn rotate_rgb(src: &[u8], w: usize, h: usize, orientation: u8) -> (Vec<u8>, usize, usize) {
    let (nw, nh) = match orientation {
        5..=8 => (h, w),
        _ => (w, h),
    };
    let mut out = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match orientation {
                3 | 4 => (w - 1 - x, h - 1 - y),
                // Rotate clockwise.
                6 | 5 => (h - 1 - y, x),
                // Rotate anticlockwise.
                8 | 7 => (y, w - 1 - x),
                _ => (x, y),
            };
            let s = (y * w + x) * 3;
            let d = (ny * nw + nx) * 3;
            out[d..d + 3].copy_from_slice(&src[s..s + 3]);
        }
    }
    (out, nw, nh)
}
