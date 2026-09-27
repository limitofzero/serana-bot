//! Google Calendar's JSON, and its translation to and from the domain.
//!
//! Kept apart from the client so the shape of Google's payloads is readable in one place,
//! and so the translation can be tested without an HTTP server.

use serde::{Deserialize, Serialize};

use serana_domain::calendar::{CalendarError, CalendarEvent, EventId, NewEvent};

/// One end of an event. Google sends `dateTime` for timed events and `date` for all-day
/// ones, never both, and the distinction is the only way to tell them apart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireWhen {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireAttendee {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Google marks the authorised user's own row with this, which saves us having to know
    /// their address to find it.
    #[serde(default, rename = "self")]
    pub is_self: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub start: WireWhen,
    pub end: WireWhen,
    /// Present only on an instance of a repeating event, pointing at its master.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurring_event_id: Option<String>,
    /// Present on the master of a repeating event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recurrence: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<WireAttendee>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer: Option<WireOrganizer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireOrganizer {
    #[serde(default, rename = "self")]
    pub is_self: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct WireEvents {
    #[serde(default)]
    pub items: Vec<WireEvent>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WireToken {
    pub access_token: String,
    /// Seconds. Google has always sent this, but a missing one must not mean "never
    /// expires" — the caller treats `None` as "refresh on the next call".
    pub expires_in: Option<u64>,
}

fn parse_when(when: &WireWhen) -> Option<(jiff::Timestamp, bool)> {
    if let Some(raw) = &when.date_time {
        return raw.parse::<jiff::Timestamp>().ok().map(|at| (at, false));
    }
    // An all-day event carries a bare date. Midnight in the event's own zone is the honest
    // reading; without a zone, UTC is the only thing left.
    let date = when.date.as_ref()?.parse::<jiff::civil::Date>().ok()?;
    let midnight = date.to_datetime(jiff::civil::time(0, 0, 0, 0));
    let zone = when
        .time_zone
        .as_deref()
        .and_then(|name| jiff::tz::TimeZone::get(name).ok())
        .unwrap_or(jiff::tz::TimeZone::UTC);
    Some((midnight.to_zoned(zone).ok()?.timestamp(), true))
}

impl WireEvent {
    /// Translate, or say why not.
    ///
    /// An event without an id or without times is unusable rather than merely odd: nothing
    /// downstream can refer to it or place it on a day, so it is dropped rather than
    /// carried as a half-thing.
    pub(super) fn into_domain(self) -> Option<CalendarEvent> {
        let id = self.id?;
        let (starts_at, all_day) = parse_when(&self.start)?;
        let (ends_at, _) = parse_when(&self.end)?;

        // "Mine" means the authorised account organises it. Google marks that with
        // `organizer.self`; an event with no attendees at all is one we made for ourselves.
        let mine = self
            .organizer
            .as_ref()
            .map(|o| o.is_self)
            .unwrap_or(self.attendees.is_empty());

        Some(CalendarEvent {
            id: EventId::new(id),
            summary: self.summary.unwrap_or_default(),
            starts_at,
            ends_at,
            all_day,
            location: self.location,
            mine,
            // Either an occurrence of a series, or the series itself.
            recurring: self.recurring_event_id.is_some() || !self.recurrence.is_empty(),
        })
    }
}

impl From<&NewEvent> for WireEvent {
    fn from(event: &NewEvent) -> Self {
        // The block covers travel; the appointment itself is inside it.
        let (from, to) = event.blocked();
        Self {
            summary: Some(event.summary.clone()),
            location: event.location.clone(),
            start: WireWhen {
                date_time: Some(from.to_string()),
                ..WireWhen::default()
            },
            end: WireWhen {
                date_time: Some(to.to_string()),
                ..WireWhen::default()
            },
            ..Self::default()
        }
    }
}

/// Google's error envelope, reduced to a sentence.
pub(super) fn describe(status: u16, body: &str) -> CalendarError {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<Inner>,
    }
    #[derive(Deserialize)]
    struct Inner {
        message: Option<String>,
    }
    let message = serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|e| e.error)
        .and_then(|e| e.message)
        .unwrap_or_else(|| body.chars().take(200).collect());
    CalendarError::Unavailable(format!("{status}: {message}"))
}
