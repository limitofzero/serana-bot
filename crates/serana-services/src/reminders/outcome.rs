//! What a turn did, and how the model is told about it.

use serana_domain::reminder::Reminder;

/// What one reminder turn did, so the frontend can say so without re-deriving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReminderOutcome {
    /// `at` is the moment the turn ran. Carried so whatever renders the checklist decides
    /// what is ticked at the same instant the service did, rather than reading a clock of
    /// its own and disagreeing across a period boundary.
    Created {
        reminder: Reminder,
        at: jiff::Timestamp,
    },
    Updated {
        reminder: Reminder,
        at: jiff::Timestamp,
    },
    Deleted(Reminder),
    Acknowledged(Reminder),
    /// Items ticked off a checklist. Carries what was ticked, because a model paraphrasing
    /// the person can miss a line and the user needs to see which ones landed.
    Completed {
        reminder: Reminder,
        ticked: Vec<String>,
        /// The moment the ticking happened. Carried so whatever renders the checklist uses
        /// the same instant that decided what counts as done, instead of reading a clock of
        /// its own and disagreeing across a period boundary.
        at: jiff::Timestamp,
    },
    /// The model answered in prose instead of acting: a question back to the user when
    /// several reminders matched, or an explanation of what it could not work out. Shown
    /// verbatim, because it is the only thing that tells the user how to rephrase.
    Said(String),
}

/// What the model is told came of its call, as the tool result for the next turn.
///
/// Short on purpose: it is re-sent with every subsequent turn, so a full reminder here is
/// paid for over and over. The id and the verb are what a follow-up needs.
pub(crate) fn summarise(outcome: &ReminderOutcome) -> String {
    let (verb, reminder) = match outcome {
        ReminderOutcome::Created { reminder, .. } => ("created", reminder),
        ReminderOutcome::Updated { reminder, .. } => ("updated", reminder),
        ReminderOutcome::Deleted(r) => ("deleted", r),
        ReminderOutcome::Acknowledged(r) => ("acknowledged", r),
        ReminderOutcome::Completed {
            reminder, ticked, ..
        } => {
            return serde_json::json!({ "ok": "ticked", "id": reminder.id.as_str(), "items": ticked })
                .to_string();
        }
        ReminderOutcome::Said(words) => return words.clone(),
    };
    serde_json::json!({ "ok": verb, "id": reminder.id.as_str() }).to_string()
}
