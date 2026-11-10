use esp32_screen_host_tests::logic::touch::{Sample, TouchFilter};

fn press(x: i16, y: i16) -> Sample {
    Sample::Pressed { x, y }
}

#[test]
fn single_sample_is_noise() {
    let mut f = TouchFilter::default();
    assert_eq!(f.feed(press(100, 100)), Sample::None);
    assert_eq!(
        f.feed(Sample::Released),
        Sample::None,
        "unreported press: no release"
    );
}

#[test]
fn two_close_samples_make_a_press() {
    let mut f = TouchFilter::default();
    assert_eq!(f.feed(press(100, 100)), Sample::None);
    assert_eq!(
        f.feed(Sample::None),
        Sample::None,
        "no new sample keeps the candidate"
    );
    assert_eq!(f.feed(press(120, 90)), press(120, 90));
    // Once down, every sample goes through, wherever it is (drags).
    assert_eq!(f.feed(press(400, 300)), press(400, 300));
    assert_eq!(f.feed(Sample::Released), Sample::Released);
    assert_eq!(f.feed(Sample::Released), Sample::None);
}

#[test]
fn far_apart_samples_restart_confirmation() {
    let mut f = TouchFilter::default();
    assert_eq!(f.feed(press(10, 10)), Sample::None);
    assert_eq!(
        f.feed(press(300, 10)),
        Sample::None,
        "41+ px away: new candidate"
    );
    assert_eq!(f.feed(press(310, 20)), press(310, 20));
}

#[test]
fn confirmation_distance_is_inclusive() {
    let mut f = TouchFilter::default();
    f.feed(press(100, 100));
    assert_eq!(f.feed(press(140, 60)), press(140, 60));
}
