use esp32_screen_host_tests::logic::backlight::{duty_permille, MIN_PERMILLE};

#[test]
fn off_minimum_and_full() {
    assert_eq!(duty_permille(0), 0);
    assert_eq!(duty_permille(1), MIN_PERMILLE, "the lowest setting still lights the screen");
    assert_eq!(duty_permille(100), 1000);
    assert_eq!(duty_permille(250), 1000);
}

#[test]
fn rises_steadily() {
    let duties: Vec<u32> = (1..=100).map(duty_permille).collect();
    assert!(duties.windows(2).all(|w| w[0] <= w[1]));
    assert!(duty_permille(50) < 400, "half brightness looks half as bright, not 50% duty");
}
