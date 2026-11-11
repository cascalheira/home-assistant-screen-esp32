use esp32_screen_host_tests::logic::climate::{current, target};

#[test]
fn temperatures() {
    assert_eq!(current(18.8), "18.8°");
    assert_eq!(current(19.0), "19°");
    assert_eq!(current(-2.25), "-2.3°");
    assert_eq!(target(24.0, Some(0.5)), "24°");
    assert_eq!(target(20.5, Some(0.5)), "20.5°");
    assert_eq!(target(20.5, Some(1.0)), "21°");
}
