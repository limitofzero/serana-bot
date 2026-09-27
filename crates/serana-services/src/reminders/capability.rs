//! Reminders, as a subject a conversation can be about.
//!
//! Everything here is the part [`crate::chat::Chat`] cannot know: which tools exist, what
//! the model is told, and what each tool does to a reminder. The conversation itself — the
//! history, the alternation, compaction — is not this module's business.

use async_trait::async_trait;

use serana_domain::reminder::{ReminderId, ReminderRepository, TimeZoneName, UserId};
use serana_domain::tool::ToolSpec;
use serana_domain::{Clock, IdGenerator};

use crate::chat::Capability;

use super::error::ReminderError;
use super::outcome::{ReminderOutcome, summarise};
use super::parse::ParsedReminder;
use super::service::ReminderService;
use super::{prompt, tools};

/// The reminder capability: a [`ReminderService`] plus the zone its reminders are created
/// in.
pub struct Reminding<R, C, I> {
    reminders: ReminderService<R, C, I>,
    timezone: TimeZoneName,
}

impl<R, C, I> Reminding<R, C, I> {
    pub fn new(reminders: ReminderService<R, C, I>, timezone: TimeZoneName) -> Self {
        Self {
            reminders,
            timezone,
        }
    }

    pub fn reminders(&self) -> &ReminderService<R, C, I> {
        &self.reminders
    }

    pub fn into_reminders(self) -> ReminderService<R, C, I> {
        self.reminders
    }
}

#[async_trait]
impl<R, C, I> Capability for Reminding<R, C, I>
where
    R: ReminderRepository,
    C: Clock,
    I: IdGenerator,
{
    type Outcome = ReminderOutcome;
    type Error = ReminderError;

    fn topic(&self) -> &'static str {
        "reminders"
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
        self.reminders.now()
    }

    async fn context(&self, owner: UserId, now: jiff::Timestamp) -> Result<String, ReminderError> {
        let local = now.to_zoned(self.timezone.resolve()?);
        let existing = self.reminders.list_all(owner).await?;
        Ok(prompt::context(&local, &self.timezone, &existing, now))
    }

    async fn run(
        &self,
        owner: UserId,
        now: jiff::Timestamp,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ReminderOutcome, ReminderError> {
        let incomplete = |e| ReminderError::Unparsable(format!("the answer was incomplete: {e}"));

        match name {
            tools::CREATE => {
                let parsed: ParsedReminder =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (recurrence, text, items) = parsed.into_recurrence()?;
                Ok(ReminderOutcome::Created {
                    reminder: self
                        .reminders
                        .create(owner, now, self.timezone.clone(), recurrence, text, items)
                        .await?,
                    at: now,
                })
            }
            tools::UPDATE => {
                let args: tools::UpdateArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (recurrence, text, items) = args.schedule.into_recurrence()?;
                Ok(ReminderOutcome::Updated {
                    reminder: self
                        .reminders
                        .update(
                            owner,
                            &ReminderId::new(args.id.trim()),
                            recurrence,
                            text,
                            items,
                            now,
                        )
                        .await?,
                    at: now,
                })
            }
            tools::DELETE => {
                let args: tools::TargetArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                Ok(ReminderOutcome::Deleted(
                    self.reminders
                        .delete(owner, &ReminderId::new(args.id.trim()))
                        .await?,
                ))
            }
            tools::COMPLETE => {
                let args: tools::CompleteArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                let (reminder, ticked) = self
                    .reminders
                    .complete_items(owner, &ReminderId::new(args.id.trim()), &args.items, now)
                    .await?;
                Ok(ReminderOutcome::Completed {
                    reminder,
                    ticked,
                    at: now,
                })
            }
            tools::ACKNOWLEDGE => {
                let args: tools::TargetArgs =
                    serde_json::from_value(arguments).map_err(incomplete)?;
                Ok(ReminderOutcome::Acknowledged(
                    self.reminders
                        .acknowledge(owner, &ReminderId::new(args.id.trim()))
                        .await?,
                ))
            }
            // `knows` gates the caller, so reaching here means the two lists disagree.
            other => Err(ReminderError::Unparsable(format!(
                "I do not know how to {other}"
            ))),
        }
    }

    fn said(&self, words: String) -> ReminderOutcome {
        ReminderOutcome::Said(words)
    }

    fn summarise(&self, outcome: &ReminderOutcome) -> String {
        summarise(outcome)
    }
}
