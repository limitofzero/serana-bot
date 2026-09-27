//! The calendar, as a subject a conversation can be about.
//!
//! Everything here is the part [`crate::chat::Chat`] cannot know: which tools exist, what
//! the model is told, and what each tool does to the calendar. The conversation itself —
//! the history, the alternation, compaction — is not this module's business.

use async_trait::async_trait;

use serana_domain::Clock;
use serana_domain::calendar::{CalendarPort, EventId};
use serana_domain::reminder::UserId;
use serana_domain::tool::ToolSpec;

use crate::chat::Capability;

use super::error::CalendarTurnError;
use super::outcome::{CalendarOutcome, summarise};
use super::service::CalendarService;
use super::{prompt, tools};

/// The calendar capability.
pub struct Planning<P, C> {
    calendar: CalendarService<P, C>,
}

impl<P, C> Planning<P, C> {
    pub fn new(calendar: CalendarService<P, C>) -> Self {
        Self { calendar }
    }

    pub fn calendar(&self) -> &CalendarService<P, C> {
        &self.calendar
    }
}

#[async_trait]
impl<P, C> Capability for Planning<P, C>
where
    P: CalendarPort,
    C: Clock,
{
    type Outcome = CalendarOutcome;
    type Error = CalendarTurnError;

    fn topic(&self) -> &'static str {
        "calendar"
    }

    fn instructions(&self) -> &'static str {
        prompt::INSTRUCTIONS
    }

    fn tools(&self) -> Vec<ToolSpec> {
        tools::specs()
    }

    fn knows(&self, name: &str) -> bool {
        tools::is_known(name)
    }

    fn now(&self) -> jiff::Timestamp {
        self.calendar.now()
    }

    async fn context(
        &self,
        _owner: UserId,
        now: jiff::Timestamp,
    ) -> Result<String, CalendarTurnError> {
        let timezone = self.calendar.timezone();
        let local = now.to_zoned(timezone.resolve()?);
        let week = self.calendar.upcoming(now).await?;
        Ok(prompt::context(&local, timezone, &week))
    }

    async fn run(
        &self,
        _owner: UserId,
        _now: jiff::Timestamp,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CalendarOutcome, CalendarTurnError> {
        let incomplete =
            |e| CalendarTurnError::Unparsable(format!("the answer was incomplete: {e}"));

        match name {
            tools::LIST => {
                let args: tools::RangeArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (events, from, to) = self
                    .calendar
                    .between(date(&args.from)?, date(&args.to)?)
                    .await?;
                Ok(CalendarOutcome::Listed { events, from, to })
            }
            tools::CREATE => {
                let args: tools::NewEventArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let end = args.end_time.as_deref().map(time).transpose()?;
                let (event, travel) = self
                    .calendar
                    .create(
                        &args.summary,
                        date(&args.date)?,
                        time(&args.start_time)?,
                        end,
                        args.location
                            .map(|raw| raw.trim().to_owned())
                            .filter(|raw| !raw.is_empty()),
                        args.in_person,
                    )
                    .await?;
                Ok(CalendarOutcome::Created { event, travel })
            }
            tools::CANCEL => {
                let args: tools::TargetArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (event, how) = self.calendar.cancel(&EventId::new(args.id.trim())).await?;
                Ok(CalendarOutcome::Cancelled { event, how })
            }
            tools::DELETE => {
                let args: tools::TargetArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                Ok(CalendarOutcome::Deleted(
                    self.calendar.delete(&EventId::new(args.id.trim())).await?,
                ))
            }
            // `knows` gates the caller, so reaching here means the two lists disagree.
            other => Err(CalendarTurnError::Unparsable(format!(
                "I do not know how to {other}"
            ))),
        }
    }

    fn said(&self, words: String) -> CalendarOutcome {
        CalendarOutcome::Said(words)
    }

    fn summarise(&self, outcome: &CalendarOutcome) -> String {
        summarise(outcome)
    }
}

/// Model output is untrusted input: a date is parsed, never assumed.
fn date(raw: &str) -> Result<jiff::civil::Date, CalendarTurnError> {
    raw.trim()
        .parse()
        .map_err(|e| CalendarTurnError::Unparsable(format!("{raw:?} is not a date: {e}")))
}

fn time(raw: &str) -> Result<jiff::civil::Time, CalendarTurnError> {
    let raw = raw.trim();
    // `HH:MM` is what the schema asks for, but `HH:MM:SS` is what a model occasionally
    // sends, and refusing it would be pedantry rather than safety.
    raw.parse()
        .map_err(|e| CalendarTurnError::Unparsable(format!("{raw:?} is not a time of day: {e}")))
}
