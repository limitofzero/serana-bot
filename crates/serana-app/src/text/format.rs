//! Turning numbers and dates into the words a person would use.
//!
//! Shared by every kind of reply, so "20" reads as "20th" and a date reads the same way
//! whether it belongs to a reminder or to an appointment.

/// Join phrases the way a person would: "a", "a and b", "a, b and c".
pub(super) fn list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

pub(super) const MONTHS: [&str; 12] = [
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

pub(super) fn hhmm(time: jiff::civil::Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// `20` becomes `20th`, `21` becomes `21st`.
///
/// The teens are the exception the naive rule gets wrong: 11, 12 and 13 take "th" despite
/// ending in 1, 2 and 3.
pub(super) fn ordinal(day: i8) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

pub(super) fn day_and_month(date: jiff::civil::Date) -> String {
    // `month()` is 1-12, so the index is always in range.
    format!(
        "{} {}",
        date.day(),
        MONTHS[usize::from(date.month() as u8) - 1]
    )
}
