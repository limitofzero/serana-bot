use std::time::Duration;

use crate::reminder::{Recurrence, TimeZoneName, UserId};

use super::*;

const OWNER: UserId = UserId::new(42);

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn quote(symbol: &str, usd: f64) -> Quote {
    Quote {
        asset: AssetId::new(symbol.to_lowercase()),
        symbol: symbol.into(),
        usd,
        change_24h: None,
    }
}

fn snapshot(at: &str) -> Snapshot {
    Snapshot::new(vec![quote("BTC", 84_672.0)], ts(at), "coingecko")
}

fn digest() -> Digest {
    Digest::new(
        OWNER,
        Recurrence::Daily {
            at: jiff::civil::time(9, 0, 0, 0),
        },
        TimeZoneName::new("Asia/Tbilisi"),
        vec![
            AssetId::new("bitcoin"),
            AssetId::new("ethereum"),
            AssetId::new("cow-protocol"),
        ],
    )
}

#[test]
fn a_new_digest_is_not_scheduled_until_it_is_rescheduled() {
    // The same bargain reminders make: nothing fires off the back of construction alone.
    assert!(!digest().is_active());
}

#[test]
fn the_schedule_lands_at_the_local_hour_asked_for() {
    // 09:00 in Tbilisi is 05:00Z, and the person only ever thinks in the former.
    let mut digest = digest();
    let next = digest.reschedule(ts("2026-09-27T19:00:00Z")).unwrap();
    assert_eq!(next, Some(ts("2026-09-28T05:00:00Z")));
    assert!(digest.is_active());
}

#[test]
fn sending_records_the_prices_and_moves_to_the_next_day() {
    let mut digest = digest();
    let now = ts("2026-09-28T05:00:00Z");
    let next = digest
        .mark_sent(now, snapshot("2026-09-28T05:00:00Z"))
        .unwrap();

    assert_eq!(digest.last_sent_at, Some(now));
    assert_eq!(
        next,
        Some(ts("2026-09-29T05:00:00Z")),
        "tomorrow, not today"
    );
    assert_eq!(digest.cached.as_ref().unwrap().quotes[0].symbol, "BTC");
}

#[test]
fn what_was_read_this_morning_is_not_stale_this_evening() {
    // The whole point of a digest: the numbers are already here when you ask.
    let mut digest = digest();
    digest.cached = Some(snapshot("2026-09-28T05:00:00Z"));
    assert!(!digest.is_stale(ts("2026-09-28T19:00:00Z"), Duration::from_secs(24 * 3600)));
}

#[test]
fn a_digest_that_never_ran_is_stale_rather_than_empty() {
    // Nothing cached means the scheduled run has not happened yet, so asking must read.
    assert!(digest().is_stale(ts("2026-09-28T19:00:00Z"), Duration::from_secs(24 * 3600)));
}

#[test]
fn prices_from_before_a_run_that_was_missed_are_stale() {
    // Three days of downtime must not render as today's market.
    let mut digest = digest();
    digest.cached = Some(snapshot("2026-09-25T05:00:00Z"));
    assert!(digest.is_stale(ts("2026-09-28T05:00:00Z"), Duration::from_secs(24 * 3600)));
}

#[test]
fn a_snapshot_stamped_in_the_future_has_no_age_rather_than_a_negative_one() {
    // Our clock and the source's need not agree to the second.
    let snapshot = snapshot("2026-09-28T05:00:10Z");
    assert_eq!(snapshot.age(ts("2026-09-28T05:00:00Z")), Duration::ZERO);
}

#[test]
fn a_missing_24_hour_move_is_absent_rather_than_zero() {
    // Rendering "0.0%" for a source that simply does not report the move would be an
    // invention, and the one that matters most — it reads as a flat market.
    assert_eq!(quote("COW", 0.161).change_24h, None);
}

#[test]
fn a_digest_survives_a_round_trip_through_json() {
    // It is stored as an opaque JSON column, cache and all.
    let mut original = digest();
    original.reschedule(ts("2026-09-27T19:00:00Z")).unwrap();
    original.cached = Some(snapshot("2026-09-27T05:00:00Z"));

    let json = serde_json::to_string(&original).unwrap();
    assert_eq!(serde_json::from_str::<Digest>(&json).unwrap(), original);
}

#[test]
fn a_digest_stored_before_there_was_a_cache_still_loads() {
    // The field arrived after the first rows did; a missing one is "nothing read yet".
    let json = serde_json::json!({
        "owner": 42,
        "recurrence": { "kind": "daily", "at": "09:00:00" },
        "timezone": "Asia/Tbilisi",
        "assets": ["bitcoin"],
        "next_fire_at": null,
        "last_sent_at": null,
    })
    .to_string();
    let digest: Digest = serde_json::from_str(&json).expect("loads without `cached`");
    assert!(digest.cached.is_none());
}

#[test]
fn watching_something_new_adds_it_at_the_end_and_drops_the_cache() {
    // The cache no longer covers what is watched. Showing it would present a list missing
    // the very asset just asked for, for a whole day.
    let mut digest = digest();
    digest.cached = Some(snapshot("2026-09-28T05:00:00Z"));

    assert!(digest.watch(AssetId::new("1inch")));
    assert_eq!(digest.assets.last(), Some(&AssetId::new("1inch")));
    assert!(digest.cached.is_none());
}

#[test]
fn watching_something_already_watched_changes_nothing() {
    let mut digest = digest();
    digest.cached = Some(snapshot("2026-09-28T05:00:00Z"));

    assert!(!digest.watch(AssetId::new("bitcoin")));
    assert_eq!(digest.assets.len(), 3, "no duplicate row");
    assert!(digest.cached.is_some(), "and nothing to invalidate");
}

#[test]
fn unwatching_keeps_the_rest_of_the_cache() {
    // Everything else in it is still true; reading the market again to show one line
    // fewer would be waste.
    let mut digest = digest();
    digest.cached = Some(Snapshot::new(
        vec![
            Quote {
                asset: AssetId::new("bitcoin"),
                symbol: "BTC".into(),
                usd: 84_753.0,
                change_24h: None,
            },
            Quote {
                asset: AssetId::new("cow-protocol"),
                symbol: "COW".into(),
                usd: 0.16,
                change_24h: None,
            },
        ],
        ts("2026-09-28T05:00:00Z"),
        "coingecko",
    ));

    assert!(digest.unwatch(&AssetId::new("cow-protocol")));
    assert!(!digest.assets.contains(&AssetId::new("cow-protocol")));
    let cached = digest.cached.unwrap();
    assert_eq!(cached.quotes.len(), 1);
    assert_eq!(cached.quotes[0].symbol, "BTC");
}

#[test]
fn unwatching_something_not_watched_says_so() {
    assert!(!digest().unwatch(&AssetId::new("1inch")));
}

#[test]
fn a_watched_asset_is_found_by_its_ticker_as_well_as_its_id() {
    // People say "remove COW", not "remove cow-protocol".
    let mut digest = digest();
    digest.cached = Some(Snapshot::new(
        vec![Quote {
            asset: AssetId::new("cow-protocol"),
            symbol: "COW".into(),
            usd: 0.16,
            change_24h: None,
        }],
        ts("2026-09-28T05:00:00Z"),
        "coingecko",
    ));

    assert_eq!(digest.find("cow"), Some(&AssetId::new("cow-protocol")));
    assert_eq!(
        digest.find("Cow-Protocol"),
        Some(&AssetId::new("cow-protocol"))
    );
    assert_eq!(digest.find("1inch"), None);
}
