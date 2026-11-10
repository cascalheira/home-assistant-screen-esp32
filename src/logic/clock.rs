//! What the screensaver clock shows: the four digit cards and the date line.

/// The digit on each card for `hour:minute`. With `h24` false the hour is
/// 1-12 and a leading zero becomes -1 (a blank card).
pub fn digits(hour: i32, minute: i32, h24: bool) -> [i32; 4] {
    let hour = if h24 {
        hour
    } else {
        match hour % 12 {
            0 => 12,
            h => h,
        }
    };
    let first = if !h24 && hour < 10 { -1 } else { hour / 10 };
    [first, hour % 10, minute / 10, minute % 10]
}

/// Calendar fields as in C's `struct tm`.
#[derive(Clone, Copy, Debug)]
pub struct Date {
    /// 0 = Sunday.
    pub weekday: i32,
    /// 1-31.
    pub day: i32,
    /// 0 = January.
    pub month: i32,
    /// 0-23.
    pub hour: i32,
}

/// "Tuesday, 10 November" / "Terça-feira, 10 de novembro", plus " · AM" or
/// " · PM" on a 12-hour clock.
pub fn date_line(date: Date, portuguese: bool, h24: bool) -> String {
    let (days, months) = if portuguese {
        (DAYS_PT, MONTHS_PT)
    } else {
        (DAYS_EN, MONTHS_EN)
    };
    let day = days.get(date.weekday as usize).copied().unwrap_or("");
    let month = months.get(date.month as usize).copied().unwrap_or("");
    let text = if portuguese {
        format!("{day}, {} de {month}", date.day)
    } else {
        format!("{day}, {} {month}", date.day)
    };
    if h24 {
        text
    } else {
        format!("{text} · {}", if date.hour < 12 { "AM" } else { "PM" })
    }
}

const DAYS_EN: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS_PT: [&str; 7] = [
    "Domingo",
    "Segunda-feira",
    "Terça-feira",
    "Quarta-feira",
    "Quinta-feira",
    "Sexta-feira",
    "Sábado",
];
const MONTHS_PT: [&str; 12] = [
    "janeiro",
    "fevereiro",
    "março",
    "abril",
    "maio",
    "junho",
    "julho",
    "agosto",
    "setembro",
    "outubro",
    "novembro",
    "dezembro",
];
