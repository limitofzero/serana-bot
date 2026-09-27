//! Everything the bot says about prices.
//!
//! Tickers come from a public market API and are echoed exactly as they came. Nothing here
//! reads them as anything but text, and nothing here produces a number the source did not.

use serana_domain::prices::{Digest, Quote, Snapshot};
use serana_domain::reminder::TimeZoneName;
use serana_services::PriceOutcome;

use super::format::day_and_month;
use super::reminders::describe;

/// Shown when no source would answer and there is nothing stored to fall back on.
pub const NO_MARKET: &str = "Could not reach the market just now, and there is nothing \
     stored yet. Try again in a minute.";

/// A price, to as many places as it is worth reading.
///
/// A rule rather than a fixed precision: the same digest holds a five-figure number and one
/// that lives in its fourth decimal place. "$84,753.0000" and "$0.16" are both wrong, and
/// they are wrong in the same message.
fn usd(price: f64) -> String {
    // Roughly six significant figures, which is where a price stops carrying information:
    // Bitcoin's cents are noise, Ether's are not, and a token in its fourth decimal place
    // has nothing else.
    let places = match price.abs() {
        p if p >= 10_000.0 => 0,
        p if p >= 1.0 => 2,
        p if p >= 0.01 => 4,
        _ => 6,
    };
    format!("${}", group(&format!("{price:.places$}")))
}

/// Thousands separators, so a five-figure price is read at a glance.
fn group(rendered: &str) -> String {
    let (whole, rest) = match rendered.split_once('.') {
        Some((whole, fraction)) => (whole, format!(".{fraction}")),
        None => (rendered, String::new()),
    };
    let (sign, digits) = match whole.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("", whole),
    };
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{sign}{grouped}{rest}")
}

/// The 24-hour move, or nothing at all.
///
/// A source that does not report the move renders as silence, never as "0.00%" — a flat
/// market and an unknown one must not look the same.
fn move_24h(change: Option<f64>) -> String {
    match change {
        None => String::new(),
        Some(change) if change >= 0.0 => format!(" ▲ {change:.2}%"),
        Some(change) => format!(" ▼ {:.2}%", change.abs()),
    }
}

fn row(quote: &Quote) -> String {
    format!(
        "• {} — {}{}",
        quote.symbol,
        usd(quote.usd),
        move_24h(quote.change_24h)
    )
}

/// How long ago these were read, in the words someone would use.
fn freshness(snapshot: &Snapshot, now: jiff::Timestamp, zone: &TimeZoneName) -> String {
    let minutes = snapshot.age(now).as_secs() / 60;
    match minutes {
        0 => "just now".to_owned(),
        1 => "a minute ago".to_owned(),
        2..=59 => format!("{minutes} minutes ago"),
        60..=119 => "an hour ago".to_owned(),
        120..=1439 => format!("{} hours ago", minutes / 60),
        _ => {
            let local = snapshot
                .taken_at
                .to_zoned(zone.resolve().unwrap_or(jiff::tz::TimeZone::UTC));
            format!(
                "on {} at {}",
                day_and_month(local.date()),
                local.time().strftime("%H:%M")
            )
        }
    }
}

/// When the next digest lands, in the person's own zone.
fn next_digest(digest: &Digest) -> String {
    let Some(at) = digest.next_fire_at else {
        return "🔕 Not arriving — say \"start sending it\" to turn it back on.".to_owned();
    };
    let Ok(tz) = digest.timezone.resolve() else {
        return format!("🗓 Next: {at}");
    };
    let local = at.to_zoned(tz);
    format!(
        "🗓 Next: {} at {} · {}",
        day_and_month(local.date()),
        local.time().strftime("%H:%M"),
        describe(&digest.recurrence)
    )
}

/// The digest itself.
pub fn digest(snapshot: &Snapshot, now: jiff::Timestamp, zone: &TimeZoneName) -> String {
    if snapshot.is_empty() {
        return NO_MARKET.to_owned();
    }
    let rows: Vec<String> = snapshot.quotes.iter().map(row).collect();
    format!(
        "💰 Prices · {}\n{}",
        freshness(snapshot, now, zone),
        rows.join("\n")
    )
}

/// The digest as it arrives unprompted.
///
/// Read at the moment it is sent, so there is no age to report and no zone to report it in
/// — asking for either would be asking the caller to invent one.
pub fn pushed(snapshot: &Snapshot) -> String {
    digest(snapshot, snapshot.taken_at, &TimeZoneName::new("UTC"))
}

/// The digest in reply to a question, with what happens next underneath it.
fn shown(digest_: &Digest, snapshot: &Snapshot, now: jiff::Timestamp) -> String {
    let body = digest(snapshot, now, &digest_.timezone);
    if snapshot.is_empty() {
        return body;
    }
    format!("{body}\n\n{}", next_digest(digest_))
}

/// Confirmation after moving the digest.
pub fn rescheduled(digest: &Digest) -> String {
    format!("✅ Digest moved.\n{}", next_digest(digest))
}

/// Confirmation after switching the digest off.
pub fn paused(_digest: &Digest) -> String {
    "🔕 The daily digest is off. Ask any time with /prices — what was last read is still \
     here."
        .to_owned()
}

/// What to say about a finished price turn.
pub fn outcome(outcome: &PriceOutcome, now: jiff::Timestamp) -> String {
    match outcome {
        PriceOutcome::Showed {
            digest, snapshot, ..
        } => shown(digest, snapshot, now),
        PriceOutcome::Rescheduled(digest) => rescheduled(digest),
        PriceOutcome::Paused(digest) => paused(digest),
        PriceOutcome::Said(words) => words.clone(),
    }
}

#[cfg(test)]
mod tests;
