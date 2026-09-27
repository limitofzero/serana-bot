//! Round-trips through a real database. Every query here is written with the runtime API,
//! so a column typo is a test failure rather than a compile error.

use serana_domain::prices::{AssetId, Quote, Snapshot};
use serana_domain::reminder::{Recurrence, TimeZoneName};

use super::*;

const OWNER: UserId = UserId::new(42);
const OTHER: UserId = UserId::new(7);

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn digest(owner: UserId) -> Digest {
    Digest::new(
        owner,
        Recurrence::Daily {
            at: jiff::civil::time(9, 0, 0, 0),
        },
        TimeZoneName::new("Asia/Tbilisi"),
        vec![AssetId::new("bitcoin"), AssetId::new("cow-protocol")],
    )
}

async fn repository() -> SqliteDigestRepository {
    SqliteDigestRepository::in_memory().await.unwrap()
}

#[tokio::test]
async fn a_digest_round_trips_with_its_schedule_and_its_watchlist() {
    let repository = repository().await;
    let mut stored = digest(OWNER);
    stored.reschedule(ts("2026-09-27T19:00:00Z")).unwrap();

    repository.put(&stored).await.unwrap();
    assert_eq!(repository.get(OWNER).await.unwrap(), Some(stored));
}

#[tokio::test]
async fn the_cached_prices_survive_storage() {
    // The point of the cache is that asking costs nothing after a restart, not merely
    // in-process.
    let repository = repository().await;
    let mut stored = digest(OWNER);
    stored
        .mark_sent(
            ts("2026-09-28T05:00:00Z"),
            Snapshot::new(
                vec![Quote {
                    asset: AssetId::new("cow-protocol"),
                    symbol: "COW".into(),
                    usd: 0.161014,
                    change_24h: Some(5.1075),
                }],
                ts("2026-09-28T05:00:00Z"),
                "coingecko",
            ),
        )
        .unwrap();
    repository.put(&stored).await.unwrap();

    let loaded = repository.get(OWNER).await.unwrap().unwrap();
    let cached = loaded.cached.expect("prices came back");
    assert_eq!(cached.quotes[0].usd, 0.161014, "to the last digit");
    assert_eq!(cached.quotes[0].change_24h, Some(5.1075));
    assert_eq!(cached.source, "coingecko");
}

#[tokio::test]
async fn putting_the_same_owner_twice_replaces_rather_than_duplicates() {
    // One standing order per person: there is no id, so the owner is the key.
    let repository = repository().await;
    repository.put(&digest(OWNER)).await.unwrap();

    let mut changed = digest(OWNER);
    changed.assets = vec![AssetId::new("ethereum")];
    repository.put(&changed).await.unwrap();

    let loaded = repository.get(OWNER).await.unwrap().unwrap();
    assert_eq!(loaded.assets, vec![AssetId::new("ethereum")]);
}

#[tokio::test]
async fn nobody_else_s_digest_comes_back() {
    let repository = repository().await;
    repository.put(&digest(OWNER)).await.unwrap();
    assert_eq!(repository.get(OTHER).await.unwrap(), None);
}

#[tokio::test]
async fn only_what_is_due_is_returned_and_in_order() {
    let repository = repository().await;
    let mut early = digest(OWNER);
    early.next_fire_at = Some(ts("2026-09-28T05:00:00Z"));
    let mut later = digest(OTHER);
    later.next_fire_at = Some(ts("2026-09-28T06:00:00Z"));
    repository.put(&later).await.unwrap();
    repository.put(&early).await.unwrap();

    let due = repository.due_at(ts("2026-09-28T05:30:00Z")).await.unwrap();
    assert_eq!(due.len(), 1, "{due:?}");
    assert_eq!(due[0].owner, OWNER);

    let both = repository.due_at(ts("2026-09-28T07:00:00Z")).await.unwrap();
    assert_eq!(
        both.iter().map(|d| d.owner).collect::<Vec<_>>(),
        vec![OWNER, OTHER],
        "soonest first"
    );
}

#[tokio::test]
async fn a_switched_off_digest_is_never_due() {
    // `next_fire_at` NULL is how "off" is stored, and the partial index exists so those
    // rows are not even scanned.
    let repository = repository().await;
    let mut off = digest(OWNER);
    off.next_fire_at = None;
    repository.put(&off).await.unwrap();

    assert!(
        repository
            .due_at(ts("2036-01-01T00:00:00Z"))
            .await
            .unwrap()
            .is_empty()
    );
}
