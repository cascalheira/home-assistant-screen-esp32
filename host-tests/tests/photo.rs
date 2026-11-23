//! Photo helpers: EXIF orientation, decode scale, sizes, rotation.

use esp32_screen_host_tests::logic::image::{
    jpeg_orientation, jpeg_scale, photo_box, photo_fills, rotate_rgb,
};

/// A JPEG start with an EXIF segment holding only the orientation.
fn exif_jpeg(orientation: u16, little_endian: bool) -> Vec<u8> {
    let mut tiff = Vec::new();
    let w16 = |v: u16, t: &mut Vec<u8>| {
        t.extend(if little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        })
    };
    let w32 = |v: u32, t: &mut Vec<u8>| {
        t.extend(if little_endian {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        })
    };
    tiff.extend(if little_endian { b"II" } else { b"MM" });
    w16(42, &mut tiff);
    w32(8, &mut tiff);
    w16(1, &mut tiff); // one entry
    w16(0x0112, &mut tiff);
    w16(3, &mut tiff); // SHORT
    w32(1, &mut tiff);
    w16(orientation, &mut tiff);
    w16(0, &mut tiff);
    w32(0, &mut tiff); // no next IFD
    let mut body = b"Exif\0\0".to_vec();
    body.extend(tiff);
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0]; // a JFIF-ish APP0 first
    jpeg.extend([0xFF, 0xE1]);
    jpeg.extend(((body.len() + 2) as u16).to_be_bytes());
    jpeg.extend(body);
    jpeg.extend([0xFF, 0xDA, 0, 2]);
    jpeg
}

#[test]
fn orientation_from_exif() {
    assert_eq!(jpeg_orientation(&exif_jpeg(6, true)), 6);
    assert_eq!(jpeg_orientation(&exif_jpeg(8, false)), 8);
    assert_eq!(jpeg_orientation(&exif_jpeg(3, true)), 3);
    assert_eq!(jpeg_orientation(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]), 1);
    assert_eq!(jpeg_orientation(b"not a jpeg"), 1);
    assert_eq!(jpeg_orientation(&exif_jpeg(6, true)[..20]), 1, "truncated");
}

#[test]
fn decode_scale() {
    // A 12 MP camera photo for a 1024x600 screen: 1/4 (1000x750) still
    // fills the height.
    assert_eq!(jpeg_scale(4000, 3000, (1024, 600), 4_000_000), Some(2));
    // A photo already the screen's size is not reduced.
    assert_eq!(jpeg_scale(1024, 600, (1024, 600), 4_000_000), Some(0));
    // Memory forces more reduction than filling the screen allows.
    assert_eq!(jpeg_scale(4000, 3000, (1024, 600), 200_000), Some(3));
    assert_eq!(jpeg_scale(40000, 30000, (1024, 600), 200_000), None);
}

#[test]
fn photo_size_to_ask_for() {
    assert_eq!(photo_box((1024, 600), 10_000_000), Some((1024, 600)));
    let (w, h) = photo_box((800, 480), 200_000).unwrap();
    assert!(w * h <= 200_000 && w % 16 == 0 && w >= 560, "{w}x{h}");
    assert_eq!(photo_box((800, 480), 20_000), None);
}

#[test]
fn fill_or_show_whole() {
    assert!(photo_fills(1024, 600, (1024, 600)));
    assert!(
        photo_fills(4000, 3000, (1024, 600)),
        "4:3 on a 16:9-ish screen"
    );
    assert!(!photo_fills(3000, 4000, (1024, 600)), "portrait");
    assert!(!photo_fills(0, 10, (1024, 600)));
}

#[test]
fn rotation() {
    // 2x1: red, green.
    let src = [255, 0, 0, 0, 255, 0];
    let (cw, w, h) = rotate_rgb(&src, 2, 1, 6);
    assert_eq!((w, h), (1, 2));
    assert_eq!(cw, vec![255, 0, 0, 0, 255, 0], "clockwise: red on top");
    let (acw, ..) = rotate_rgb(&src, 2, 1, 8);
    assert_eq!(
        acw,
        vec![0, 255, 0, 255, 0, 0],
        "anticlockwise: green on top"
    );
    let (flip, ..) = rotate_rgb(&src, 2, 1, 3);
    assert_eq!(flip, vec![0, 255, 0, 255, 0, 0]);
}
