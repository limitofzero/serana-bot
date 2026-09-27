//! What a price turn did, and how the model is told about it.

use serana_domain::prices::{AssetMatch, Digest, Snapshot};

/// What a change to the watchlist did, name by name.
///
/// Every name asked about lands in exactly one list, so the reply can account for all of
/// them. A model paraphrasing "add uni, aave and foobar" can drop one, and the person needs
/// to see which ones landed.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistEdit {
    pub digest: Digest,
    /// Newly watched, as the lookup resolved them — so the reply can say *which* asset
    /// "cow" turned out to be.
    pub added: Vec<AssetMatch>,
    /// Asked for, but already there.
    pub already: Vec<AssetMatch>,
    /// Taken off, by the label it was shown under.
    pub removed: Vec<String>,
    /// Names that matched nothing — nothing found to add, or nothing watched to remove.
    pub missing: Vec<String>,
}

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
    /// The watchlist changed. `snapshot` is the digest as it now stands, when there is one
    /// to show — read fresh after an addition, or what was cached less the removed rows.
    Edited {
        edit: WatchlistEdit,
        snapshot: Option<Snapshot>,
    },
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
        PriceOutcome::Edited { edit, .. } => serde_json::json!({
            "ok": "edited",
            "added": edit.added.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            "already_watching": edit.already.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            "removed": edit.removed,
            "not_found": edit.missing,
            "now_watching": edit.digest.assets.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
        })
        .to_string(),
        PriceOutcome::Said(words) => words.clone(),
    }
}
