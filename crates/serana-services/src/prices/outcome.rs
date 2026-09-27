//! What a price turn did, and how the model is told about it.

use serana_domain::prices::{Digest, Snapshot};

/// What one price turn did, so the frontend can say so without re-deriving it.
#[derive(Debug, Clone, PartialEq)]
pub enum PriceOutcome {
    /// The prices. `read_now` says whether they were fetched for this reply or came from
    /// the cache — the reply words it differently, because "as of this morning" and "as of
    /// just now" are different claims.
    Showed {
        digest: Digest,
        snapshot: Snapshot,
        read_now: bool,
    },
    /// The daily digest now goes out on a different schedule.
    Rescheduled(Digest),
    /// The daily digest is switched off. Asking still works; nothing arrives unprompted.
    Paused(Digest),
    /// The model answered in prose instead of acting — a question back, most often about
    /// what time of day was meant. Shown verbatim.
    Said(String),
}

/// What the model is told came of its call, as the tool result for the next turn.
///
/// Short on purpose: it is re-sent with every subsequent turn. The prices themselves are
/// deliberately *not* repeated back — they are already in the reply the person is reading,
/// and a stale copy riding every later turn is how a model ends up quoting yesterday's
/// number as today's.
pub(crate) fn summarise(outcome: &PriceOutcome) -> String {
    match outcome {
        PriceOutcome::Showed {
            snapshot, read_now, ..
        } => serde_json::json!({
            "ok": "showed",
            "assets": snapshot.quotes.len(),
            "read_now": read_now,
            "taken_at": snapshot.taken_at.to_string(),
        })
        .to_string(),
        PriceOutcome::Rescheduled(digest) => serde_json::json!({
            "ok": "rescheduled",
            "next": digest.next_fire_at.map(|at| at.to_string()),
        })
        .to_string(),
        PriceOutcome::Paused(_) => serde_json::json!({ "ok": "paused" }).to_string(),
        PriceOutcome::Said(words) => words.clone(),
    }
}
