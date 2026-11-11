use esp32_screen_host_tests::logic::pin::{hash, matches, valid, Attempts, BLOCK_SECS, MAX_FAILURES};

#[test]
fn format() {
    assert!(valid("1234") && valid("12345678"));
    assert!(!valid("123") && !valid("123456789") && !valid("12a4") && !valid(""));
}

#[test]
fn stored_as_salted_hash() {
    let salt = [7u8; 16];
    let stored = hash(&salt, "2580");
    assert!(matches(&salt, &stored, "2580"));
    assert!(!matches(&salt, &stored, "2581"));
    assert_ne!(hash(&[8u8; 16], "2580"), stored, "the salt changes the hash");
}

#[test]
fn guessing_is_slowed_down() {
    let mut attempts = Attempts::default();
    for i in 1..MAX_FAILURES {
        assert_eq!(attempts.failed(100), None, "try {i}");
    }
    assert_eq!(attempts.left(), 1);
    assert_eq!(attempts.failed(100), Some(BLOCK_SECS));
    assert_eq!(attempts.blocked(110), Some(BLOCK_SECS - 10));
    assert_eq!(attempts.blocked(100 + BLOCK_SECS), None);
    attempts.failed(200);
    attempts.succeeded();
    assert_eq!(attempts.left(), MAX_FAILURES);
}
