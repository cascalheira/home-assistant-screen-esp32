use esp32_screen_host_tests::logic::clock::{date_line, digits, Date};

#[test]
fn digits_24_hour() {
    assert_eq!(digits(0, 5, true), [0, 0, 0, 5]);
    assert_eq!(digits(9, 41, true), [0, 9, 4, 1]);
    assert_eq!(digits(23, 59, true), [2, 3, 5, 9]);
}

#[test]
fn digits_12_hour_blank_leading_card() {
    assert_eq!(digits(0, 0, false), [1, 2, 0, 0], "midnight is 12");
    assert_eq!(digits(9, 41, false), [-1, 9, 4, 1]);
    assert_eq!(digits(12, 30, false), [1, 2, 3, 0], "noon is 12");
    assert_eq!(digits(13, 5, false), [-1, 1, 0, 5]);
    assert_eq!(digits(22, 15, false), [1, 0, 1, 5]);
}

fn date(weekday: i32, day: i32, month: i32, hour: i32) -> Date {
    Date {
        weekday,
        day,
        month,
        hour,
    }
}

#[test]
fn date_line_english() {
    assert_eq!(
        date_line(date(2, 10, 10, 9), false, true),
        "Tuesday, 10 November"
    );
    assert_eq!(
        date_line(date(0, 1, 0, 9), false, false),
        "Sunday, 1 January · AM"
    );
    assert_eq!(
        date_line(date(6, 31, 11, 12), false, false),
        "Saturday, 31 December · PM"
    );
}

#[test]
fn date_line_portuguese() {
    assert_eq!(
        date_line(date(2, 10, 10, 20), true, true),
        "Terça-feira, 10 de novembro"
    );
    assert_eq!(
        date_line(date(6, 7, 2, 23), true, false),
        "Sábado, 7 de março · PM"
    );
}

#[test]
fn date_line_out_of_range_fields_do_not_panic() {
    assert_eq!(date_line(date(9, 1, 14, 0), false, true), ", 1 ");
}
