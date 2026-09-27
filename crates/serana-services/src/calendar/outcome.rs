//! What a calendar turn did, and how the model is told about it.

use serana_domain::calendar::{CalendarEvent, Cancellation};

/// What one calendar turn did, so the frontend can say so without re-deriving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarOutcome {
    /// What is on between two moments. `from` and `to` are carried so the reply can name
    /// the span that was searched — an empty answer is only meaningful with the question.
    Listed {
        events: Vec<CalendarEvent>,
        from: jiff::Timestamp,
        to: jiff::Timestamp,
    },
    /// `travel` is what was reserved either side of the appointment, so the reply can say
    /// that the block is wider than the appointment rather than looking like a mistake.
    Created {
        event: CalendarEvent,
        travel: std::time::Duration,
    },
    /// Cancelled, and how: struck off our own calendar, or declined on somebody else's.
    /// The person asked for one thing and got one of two, so the reply says which.
    Cancelled {
        event: CalendarEvent,
        how: Cancellation,
    },
    /// Gone for good.
    Deleted(CalendarEvent),
    /// The model answered in prose instead of acting: a question back when several events
    /// matched, or the confirmation it must ask for before deleting anything. Shown
    /// verbatim.
    Said(String),
}

/// What the model is told came of its call, as the tool result for the next turn.
///
/// Short on purpose: it is re-sent with every subsequent turn. A listing is the exception —
/// the ids are the whole point of having asked, and "cancel the second one" needs them on
/// the turn after.
pub(crate) fn summarise(outcome: &CalendarOutcome) -> String {
    match outcome {
        CalendarOutcome::Listed { events, .. } => {
            let rows: Vec<serde_json::Value> = events
                .iter()
                .map(|event| {
                    serde_json::json!({
                        "id": event.id.as_str(),
                        "summary": event.summary,
                        "starts_at": event.starts_at.to_string(),
                    })
                })
                .collect();
            serde_json::json!({ "ok": "listed", "events": rows }).to_string()
        }
        CalendarOutcome::Created { event, .. } => {
            serde_json::json!({ "ok": "created", "id": event.id.as_str() }).to_string()
        }
        CalendarOutcome::Cancelled { event, how } => serde_json::json!({
            "ok": match how {
                Cancellation::StrikeOff => "struck off",
                Cancellation::Decline => "declined",
            },
            "id": event.id.as_str(),
        })
        .to_string(),
        CalendarOutcome::Deleted(event) => {
            serde_json::json!({ "ok": "deleted", "id": event.id.as_str() }).to_string()
        }
        CalendarOutcome::Said(words) => words.clone(),
    }
}
