use esp32_screen_host_tests::logic::image::descriptor;

/// An app image's first bytes with `esp_app_desc_t` filled in.
fn header(magic: u32, version: &str, project: &str) -> Vec<u8> {
    let mut h = vec![0u8; 256];
    h[0] = 0xE9; // image magic
    h[32..36].copy_from_slice(&magic.to_le_bytes());
    h[48..48 + version.len()].copy_from_slice(version.as_bytes());
    h[80..80 + project.len()].copy_from_slice(project.as_bytes());
    h
}

#[test]
fn reads_project_and_version() {
    let h = header(0xABCD_5432, "0.4.0", "esp32-screen");
    assert_eq!(
        descriptor(&h),
        Some(("esp32-screen".into(), "0.4.0".into()))
    );
}

#[test]
fn rejects_other_files() {
    assert_eq!(
        descriptor(&header(0, "0.4.0", "esp32-screen")),
        None,
        "wrong magic"
    );
    assert_eq!(descriptor(&[0u8; 100]), None, "too short");
    assert_eq!(descriptor(b"PK\x03\x04 not an image at all, just a zip file header......................................................."), None);
}

#[test]
fn full_width_fields_without_terminator() {
    let h = header(0xABCD_5432, &"v".repeat(32), &"p".repeat(32));
    let (project, version) = descriptor(&h).unwrap();
    assert_eq!((project.len(), version.len()), (32, 32));
}

#[test]
fn take_jpeg_splits_a_multipart_stream() {
    use esp32_screen_host_tests::logic::image::take_jpeg;
    let jpeg = |n: u8| vec![0xFF, 0xD8, n, 0xFF, 0x00, n, 0xFF, 0xD9];
    let mut buf = b"--frame\r\nContent-Type: image/jpeg\r\n\r\n".to_vec();
    buf.extend(jpeg(1));
    buf.extend_from_slice(b"\r\n--frame\r\nContent-Type: image/jpeg\r\n\r\n");
    buf.extend(&jpeg(2)[..4]); // second frame not complete yet
    assert_eq!(take_jpeg(&mut buf), Some(jpeg(1)));
    assert_eq!(take_jpeg(&mut buf), None);
    buf.extend(&jpeg(2)[4..]);
    assert_eq!(take_jpeg(&mut buf), Some(jpeg(2)));
    assert!(buf.is_empty());
    let mut none = b"no jpeg here".to_vec();
    assert_eq!(take_jpeg(&mut none), None);
}
