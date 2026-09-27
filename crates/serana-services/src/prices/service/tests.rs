//! What the digest does, against a price source and a store in memory.

use std::time::Duration;

use serana_domain::prices::{AssetId, PriceError};
use serana_domain::reminder::MonthDays;
use serana_testkit::{FixedClock, InMemoryDigestRepository, StubPrices};

use super::*;

const OWNER: UserId = UserId::new(42);
/// 19:00 in Tbilisi on a Sunday.
const NOW: &str = "2026-09-27T15:00:00Z";

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn assets() -> Vec<AssetId> {
    vec![AssetId::new("bitcoin"), AssetId::new("cow-protocol")]
}

fn config() -> PriceConfig {
    PriceConfig::daily(assets(), TimeZoneName::new("Asia/Tbilisi"))
}

fn source() -> StubPrices {
    StubPrices::new()
        .priced("bitcoin", 84_753.0)
        .priced("cow-protocol", 0.160295)
}

fn service(source: StubPrices) -> PriceService<StubPrices, InMemoryDigestRepository, FixedClock> {
    PriceService::new(
        source,
        InMemoryDigestRepository::new(),
        FixedClock::at(NOW),
        config(),
    )
}

#[tokio::test]
async fn asking_the_first_time_starts_the_daily_digest() {
    // Nothing is conjured at startup: a message nobody asked for is the one thing an
    // unprompted daily push must never be.
    let service = service(source());
    assert!(service.repository.is_empty());

    let digest = service.digest(OWNER).await.unwrap();
    assert!(digest.is_active());
    // 09:00 Tbilisi is 05:00Z, and 19:00 local is past it, so tomorrow.
    assert_eq!(digest.next_fire_at, Some(ts("2026-09-28T05:00:00Z")));
    assert_eq!(service.repository.len(), 1, "and it was stored");
}

#[tokio::test]
async fn the_standing_order_is_not_recreated_on_every_ask() {
    let service = service(source());
    let first = service.digest(OWNER).await.unwrap();
    service
        .set_schedule(
            OWNER,
            Recurrence::Daily {
                at: jiff::civil::time(7, 0, 0, 0),
            },
            None,
        )
        .await
        .unwrap();

    let second = service.digest(OWNER).await.unwrap();
    assert_ne!(second.recurrence, first.recurrence, "the change stuck");
    assert_eq!(service.repository.len(), 1);
}

#[tokio::test]
async fn the_first_ask_reads_the_market_and_caches_what_it_read() {
    let service = service(source());
    let (_, snapshot, read_now) = service.show(OWNER, false).await.unwrap();

    assert!(read_now);
    assert_eq!(snapshot.quotes.len(), 2);
    assert_eq!(snapshot.taken_at, ts(NOW));
    assert_eq!(service.source.calls(), 1);
}

#[tokio::test]
async fn asking_again_the_same_day_costs_nothing() {
    // The whole bargain of a digest: the scheduled run pays for the request.
    let service = service(source());
    service.show(OWNER, false).await.unwrap();
    let (_, _, read_now) = service.show(OWNER, false).await.unwrap();

    assert!(!read_now, "served from the cache");
    assert_eq!(service.source.calls(), 1, "the market was read once");
}

#[tokio::test]
async fn asking_for_fresh_prices_reads_again_even_with_a_cache() {
    let service = service(source());
    service.show(OWNER, false).await.unwrap();
    let (_, _, read_now) = service.show(OWNER, true).await.unwrap();

    assert!(read_now);
    assert_eq!(service.source.calls(), 2);
}

#[tokio::test]
async fn prices_older_than_a_day_are_read_again_without_being_asked() {
    // Stale means a scheduled run was missed. Showing three-day-old numbers as today's
    // market is the one thing this must never do.
    let service = service(source());
    let mut digest = service.digest(OWNER).await.unwrap();
    digest.cached = Some(Snapshot::new(
        Vec::new(),
        ts("2026-09-24T05:00:00Z"),
        "stub",
    ));
    service.repository.put(&digest).await.unwrap();

    let (_, snapshot, read_now) = service.show(OWNER, false).await.unwrap();
    assert!(read_now);
    assert_eq!(snapshot.taken_at, ts(NOW));
}

#[tokio::test]
async fn a_market_that_cannot_be_read_falls_back_to_what_was_cached() {
    // Yesterday's prices with an honest timestamp beat an error message.
    let service = service(source());
    service.show(OWNER, false).await.unwrap();

    let broken = PriceService::new(
        StubPrices::broken(PriceError::new("429 (rate limited)")),
        service.repository,
        FixedClock::at(NOW),
        config(),
    );
    let (_, snapshot, read_now) = broken.show(OWNER, true).await.unwrap();
    assert!(!read_now, "it is not fresh and must not claim to be");
    assert_eq!(snapshot.quotes.len(), 2);
}

#[tokio::test]
async fn a_market_that_cannot_be_read_with_nothing_cached_says_so() {
    let service = service(StubPrices::broken(PriceError::new("429 (rate limited)")));
    let error = service.show(OWNER, false).await.unwrap_err();
    assert!(matches!(error, PriceTurnError::Market(_)), "{error:?}");
}

#[tokio::test]
async fn the_schedule_can_be_moved_to_another_time() {
    let service = service(source());
    let digest = service
        .set_schedule(
            OWNER,
            Recurrence::Daily {
                at: jiff::civil::time(7, 30, 0, 0),
            },
            None,
        )
        .await
        .unwrap();
    // 07:30 Tbilisi is 03:30Z.
    assert_eq!(digest.next_fire_at, Some(ts("2026-09-28T03:30:00Z")));
}

#[tokio::test]
async fn the_schedule_can_be_moved_to_another_zone() {
    let service = service(source());
    let digest = service
        .set_schedule(
            OWNER,
            Recurrence::Daily {
                at: jiff::civil::time(9, 0, 0, 0),
            },
            Some(TimeZoneName::new("Europe/Lisbon")),
        )
        .await
        .unwrap();
    assert_eq!(digest.timezone, TimeZoneName::new("Europe/Lisbon"));
    // 09:00 in Lisbon is 08:00Z in September.
    assert_eq!(digest.next_fire_at, Some(ts("2026-09-28T08:00:00Z")));
}

#[tokio::test]
async fn a_zone_that_does_not_exist_is_refused_before_it_is_stored() {
    // Otherwise every later reschedule fails, far from the message that caused it.
    let service = service(source());
    assert!(
        service
            .set_schedule(
                OWNER,
                Recurrence::Daily {
                    at: jiff::civil::time(9, 0, 0, 0)
                },
                Some(TimeZoneName::new("Mars/Olympus")),
            )
            .await
            .is_err()
    );
    let digest = service.digest(OWNER).await.unwrap();
    assert_eq!(digest.timezone, TimeZoneName::new("Asia/Tbilisi"));
}

#[tokio::test]
async fn a_monthly_schedule_is_accepted_as_readily_as_a_daily_one() {
    // The same vocabulary reminders use, which is the whole reason it is shared.
    let service = service(source());
    let digest = service
        .set_schedule(
            OWNER,
            Recurrence::Monthly {
                days: MonthDays::new([1]).unwrap(),
                at: jiff::civil::time(9, 0, 0, 0),
            },
            None,
        )
        .await
        .unwrap();
    // The 1st at 09:00 Tbilisi.
    assert_eq!(digest.next_fire_at, Some(ts("2026-10-01T05:00:00Z")));
}

#[tokio::test]
async fn a_schedule_with_no_next_occurrence_is_refused() {
    let service = service(source());
    assert!(
        service
            .set_schedule(
                OWNER,
                Recurrence::Once {
                    at: jiff::civil::date(2020, 1, 1).to_datetime(jiff::civil::time(9, 0, 0, 0))
                },
                None,
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pausing_stops_the_digest_arriving_but_not_the_asking() {
    let service = service(source());
    service.show(OWNER, false).await.unwrap();

    let paused = service.pause(OWNER).await.unwrap();
    assert!(!paused.is_active());
    assert!(paused.cached.is_some(), "what was read is still there");

    let (_, _, read_now) = service.show(OWNER, false).await.unwrap();
    assert!(!read_now, "and asking still answers instantly");
}

#[tokio::test]
async fn resuming_puts_it_back_on_its_own_schedule() {
    let service = service(source());
    service.pause(OWNER).await.unwrap();
    let resumed = service.resume(OWNER).await.unwrap();
    assert_eq!(resumed.next_fire_at, Some(ts("2026-09-28T05:00:00Z")));
}

#[tokio::test]
async fn the_cache_window_is_a_day_by_default() {
    // Long enough that the scheduled run is what refreshes it, short enough that a missed
    // run is noticed.
    assert_eq!(config().cache_ttl, Duration::from_secs(24 * 3600));
}

#[tokio::test]
async fn one_person_s_digest_is_not_another_s() {
    let service = service(source());
    service
        .set_schedule(
            OWNER,
            Recurrence::Daily {
                at: jiff::civil::time(7, 0, 0, 0),
            },
            None,
        )
        .await
        .unwrap();

    let other = service.digest(UserId::new(7)).await.unwrap();
    assert_eq!(
        other.recurrence,
        Recurrence::Daily {
            at: jiff::civil::time(9, 0, 0, 0)
        },
        "the default, not the other person's"
    );
}
