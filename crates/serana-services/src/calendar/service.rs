//! The calendar lifecycle: what is on, putting something on, taking something off.
//!
//! No model, no conversation. Everything here is a decision that can be made from a port, a
//! clock and a zone — which is what keeps "what cancelling means" separate from "how it was
//! asked for", and what lets all of it be tested against a fake calendar.

use std::time::Duration;

use serana_domain::Clock;
use serana_domain::calendar::{CalendarEvent, CalendarPort, Cancellation, EventId, NewEvent};
use serana_domain::reminder::TimeZoneName;

use super::error::CalendarTurnError;

/// How far ahead the assistant looks when nobody asked for a span.
///
/// A week is what "what's on?" means to a person, and it is the window that makes "cancel
/// the dentist" resolvable without a lookup — the events in it ride the context line, so
/// the common request costs one model call rather than two.
pub const DEFAULT_HORIZON: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// An appointment lasts this long when the person did not say when it ends.
///
/// They usually do not — "dentist at 3" is the normal way to say it. An hour is long enough
/// to be a useful block and short enough that being wrong is cheap; the reply says what was
/// booked, so it can be corrected in the next breath.
const DEFAULT_LENGTH: Duration = Duration::from_secs(60 * 60);

/// The calendar, independent of how the request arrived.
pub struct CalendarService<P, C> {
    calendar: P,
    clock: C,
    timezone: TimeZoneName,
    /// Reserved either side of an appointment the person has to travel to.
    travel: Duration,
    horizon: Duration,
}

impl<P, C> CalendarService<P, C>
where
    P: CalendarPort,
    C: Clock,
{
    pub fn new(calendar: P, clock: C, timezone: TimeZoneName, travel: Duration) -> Self {
        Self {
            calendar,
            clock,
            timezone,
            travel,
            horizon: DEFAULT_HORIZON,
        }
    }

    pub fn now(&self) -> jiff::Timestamp {
        self.clock.now()
    }

    pub fn timezone(&self) -> &TimeZoneName {
        &self.timezone
    }

    /// What is on between now and the horizon. This is what rides the context line.
    pub async fn upcoming(
        &self,
        now: jiff::Timestamp,
    ) -> Result<Vec<CalendarEvent>, CalendarTurnError> {
        Ok(self
            .calendar
            .events_between(now, now + span(self.horizon))
            .await?)
    }

    /// What is on between two local dates, both included.
    ///
    /// Dates rather than moments because that is how the request arrives — "next week",
    /// "the 20th". The day ends at midnight local, so an event at 23:30 on the last day is
    /// in the answer.
    pub async fn between(
        &self,
        from: jiff::civil::Date,
        to: jiff::civil::Date,
    ) -> Result<(Vec<CalendarEvent>, jiff::Timestamp, jiff::Timestamp), CalendarTurnError> {
        if to < from {
            return Err(CalendarTurnError::Unparsable(
                "that range ends before it starts".into(),
            ));
        }
        let zone = self.timezone.resolve()?;
        let start = at_midnight(from, &zone)?;
        let end = at_midnight(to.tomorrow().unwrap_or(to), &zone)?;
        let events = self.calendar.events_between(start, end).await?;
        Ok((events, start, end))
    }

    /// Put an appointment on the calendar, reserving travel around it when there is
    /// somewhere to get to.
    ///
    /// The block is the appointment plus travel at both ends, so "am I free at 14:45?"
    /// answers correctly while the person is still on the road. That is the whole reason it
    /// is reserved rather than merely noted.
    pub async fn create(
        &self,
        summary: &str,
        date: jiff::civil::Date,
        start: jiff::civil::Time,
        end: Option<jiff::civil::Time>,
        location: Option<String>,
        in_person: bool,
    ) -> Result<(CalendarEvent, Duration), CalendarTurnError> {
        let summary = summary.trim();
        if summary.is_empty() {
            return Err(CalendarTurnError::Unparsable(
                "the appointment has no name".into(),
            ));
        }
        let zone = self.timezone.resolve()?;
        let starts_at = at(date, start, &zone)?;
        let ends_at = match end {
            Some(end) => at(date, end, &zone)?,
            None => starts_at + span(DEFAULT_LENGTH),
        };
        if ends_at <= starts_at {
            return Err(CalendarTurnError::Unparsable(
                "that appointment ends before it starts".into(),
            ));
        }

        let travel = if in_person {
            self.travel
        } else {
            Duration::ZERO
        };
        let event = self
            .calendar
            .create(&NewEvent {
                summary: summary.to_owned(),
                starts_at,
                ends_at,
                travel,
                location,
            })
            .await?;
        Ok((event, travel))
    }

    /// Take an event off the day without destroying it.
    ///
    /// Which of the two things that means is the calendar's to decide, not the model's: our
    /// own event is struck off, somebody else's is declined and stays on the calendar
    /// marked as not attending. Removing another person's event from their own invitation
    /// is not ours to do.
    pub async fn cancel(
        &self,
        id: &EventId,
    ) -> Result<(CalendarEvent, Cancellation), CalendarTurnError> {
        let event = self.fetch(id).await?;
        let how = event.cancellation();
        match how {
            Cancellation::StrikeOff => self.calendar.strike_off(id).await?,
            Cancellation::Decline => self.calendar.decline(id).await?,
        }
        Ok((event, how))
    }

    /// Delete for good. Irreversible, and the only thing here that is.
    pub async fn delete(&self, id: &EventId) -> Result<CalendarEvent, CalendarTurnError> {
        let event = self.fetch(id).await?;
        self.calendar.delete(id).await?;
        Ok(event)
    }

    /// The event, or a failure naming the id the model used.
    ///
    /// Read before every change, so the reply can name what was acted on and so an id the
    /// model invented is refused rather than sent to the provider.
    async fn fetch(&self, id: &EventId) -> Result<CalendarEvent, CalendarTurnError> {
        self.calendar
            .get(id)
            .await?
            .ok_or_else(|| CalendarTurnError::NotFound(id.clone()))
    }
}

/// A duration as a span. Every value used here is a whole number of seconds well inside
/// range, so the fallback is unreachable rather than merely unlikely.
fn span(duration: Duration) -> jiff::Span {
    jiff::Span::try_from(duration).unwrap_or_else(|_| jiff::Span::new())
}

fn at(
    date: jiff::civil::Date,
    time: jiff::civil::Time,
    zone: &jiff::tz::TimeZone,
) -> Result<jiff::Timestamp, CalendarTurnError> {
    date.to_datetime(time)
        .to_zoned(zone.clone())
        .map(|zoned| zoned.timestamp())
        .map_err(|e| {
            // A time that does not exist locally: the hour a daylight-saving change skips.
            CalendarTurnError::Unparsable(format!("{date} {time} is not a time here: {e}"))
        })
}

fn at_midnight(
    date: jiff::civil::Date,
    zone: &jiff::tz::TimeZone,
) -> Result<jiff::Timestamp, CalendarTurnError> {
    at(date, jiff::civil::time(0, 0, 0, 0), zone)
}

#[cfg(test)]
mod tests;
