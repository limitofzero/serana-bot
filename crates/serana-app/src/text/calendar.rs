//! Everything the bot says about the calendar.
//!
//! Event titles and locations were written by whoever created the event, and they are
//! echoed back exactly as they came — this module never paraphrases them, and never reads
//! them as anything but text.

use serana_domain::calendar::{CalendarEvent, Cancellation};
use serana_domain::reminder::TimeZoneName;
use serana_services::CalendarOutcome;

use super::format::day_and_month;

/// The marker every message naming an event carries. Deliberately not the reminders' one:
/// telling them apart is what lets a reply to a delivered reminder stay with reminders even
/// when the last thing said was about the calendar. See [`super::topic_of`].
pub(super) const ID: &str = "📆";

pub const CALENDAR_NEEDS_TEXT: &str = "Say what you want from your calendar.\n\
     For example: /calendar what have I got on Friday?";

pub const NOT_CONNECTED: &str = "No calendar is connected.\n\
     Set the Google variables in .env and restart — see .env.example.";

/// An event's moment in the person's own zone. A zone that will not resolve is a
/// misconfiguration, not this function's problem: UTC keeps the reply readable.
fn local(at: jiff::Timestamp, zone: &TimeZoneName) -> jiff::Zoned {
    at.to_zoned(zone.resolve().unwrap_or(jiff::tz::TimeZone::UTC))
}

fn hhmm(at: &jiff::Zoned) -> String {
    at.time().strftime("%H:%M").to_string()
}

/// "Friday 26 September", the way it would be said aloud.
fn full_day(at: &jiff::Zoned) -> String {
    format!("{} {}", at.strftime("%A"), day_and_month(at.date()))
}

/// The hours an event occupies, or that it takes the whole day.
fn hours(event: &CalendarEvent, zone: &TimeZoneName) -> String {
    if event.all_day {
        return "all day".to_owned();
    }
    format!(
        "{}–{}",
        hhmm(&local(event.starts_at, zone)),
        hhmm(&local(event.ends_at, zone))
    )
}

/// One event, indented under the day it falls on.
fn row(event: &CalendarEvent, zone: &TimeZoneName) -> String {
    let mut out = format!("  {}  {}\n", hours(event, zone), event.summary);
    if let Some(place) = event
        .location
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        out.push_str(&format!("     📍 {place}\n"));
    }
    out
}

/// What is on between two moments, a day at a time.
///
/// Grouped by day because that is how the question was asked. `to` is the exclusive end —
/// midnight after the last day — so the heading names the day before it rather than a day
/// nobody asked about.
pub fn listed(
    events: &[CalendarEvent],
    from: jiff::Timestamp,
    to: jiff::Timestamp,
    zone: &TimeZoneName,
) -> String {
    let first = local(from, zone).date();
    let last = local(to - jiff::Span::new().seconds(1), zone).date();
    let span = if first == last {
        full_day(&local(from, zone))
    } else {
        format!("{} – {}", day_and_month(first), day_and_month(last))
    };

    if events.is_empty() {
        return format!("📅 {span}\n\nNothing on.");
    }

    let mut out = format!("📅 {span}\n");
    let mut day = None;
    for event in events {
        let starts = local(event.starts_at, zone);
        if day != Some(starts.date()) {
            out.push_str(&format!("\n{}\n", full_day(&starts)));
            day = Some(starts.date());
        }
        out.push_str(&row(event, zone));
    }
    out
}

/// Confirmation after putting something on the calendar.
///
/// The appointment's own hours are what the person said; the reserved block is wider when
/// they have to travel. Both are shown, because a block that silently starts half an hour
/// early looks like a mistake when they next open the calendar.
pub fn created(event: &CalendarEvent, travel: std::time::Duration, zone: &TimeZoneName) -> String {
    let blocked = hours(event, zone);
    let mut out = format!(
        "✅ {}\n🗓 {}",
        event.summary,
        full_day(&local(event.starts_at, zone))
    );

    if travel.is_zero() {
        out.push_str(&format!(", {blocked}"));
    } else {
        let span = jiff::Span::try_from(travel).unwrap_or_else(|_| jiff::Span::new());
        let starts = local(event.starts_at + span, zone);
        let ends = local(event.ends_at - span, zone);
        out.push_str(&format!(", {}–{}", hhmm(&starts), hhmm(&ends)));
        out.push_str(&format!(
            "\n🚗 {} min either side for the road — {blocked} is blocked",
            travel.as_secs() / 60
        ));
    }
    if let Some(place) = event
        .location
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        out.push_str(&format!("\n📍 {place}"));
    }
    out.push_str(&format!("\n{ID} {}", event.id));
    out
}

/// Confirmation after taking something off the day.
///
/// Which of the two things happened is spelled out: the person asked for one thing and got
/// one of two, and "declined" leaving it visible on the calendar is surprising otherwise.
pub fn cancelled(event: &CalendarEvent, how: Cancellation, zone: &TimeZoneName) -> String {
    let when = format!(
        "{}, {}",
        full_day(&local(event.starts_at, zone)),
        hours(event, zone)
    );
    let what = match how {
        Cancellation::StrikeOff if event.recurring => {
            "🚫 Off — just this one; the rest of the series stands."
        }
        Cancellation::StrikeOff => "🚫 Off — taken off the day.",
        Cancellation::Decline => {
            "🚫 Declined — it stays on your calendar, marked as not attending."
        }
    };
    format!("{what}\n{}\n🗓 {when}", event.summary)
}

/// Confirmation after deleting an event for good.
pub fn deleted(event: &CalendarEvent, zone: &TimeZoneName) -> String {
    format!(
        "🗑 Deleted — {}\n🗓 {}, {}",
        event.summary,
        full_day(&local(event.starts_at, zone)),
        hours(event, zone)
    )
}

/// What to say about a finished calendar turn.
pub fn outcome(outcome: &CalendarOutcome, zone: &TimeZoneName) -> String {
    match outcome {
        CalendarOutcome::Listed { events, from, to } => listed(events, *from, *to, zone),
        CalendarOutcome::Created { event, travel } => created(event, *travel, zone),
        CalendarOutcome::Cancelled { event, how } => cancelled(event, *how, zone),
        CalendarOutcome::Deleted(event) => deleted(event, zone),
        CalendarOutcome::Said(words) => words.clone(),
    }
}

#[cfg(test)]
mod tests;
