//! The send loop, against a source, a store and a notifier in memory.

use serana_domain::prices::{AssetId, PriceError};
use serana_domain::reminder::{Recurrence, TimeZoneName, UserId};
use serana_testkit::{FixedClock, InMemoryDigestRepository, RecordingDigestNotifier, StubPrices};

use super::*;

const OWNER: UserId = UserId::new(42);
/// 09:00 in Tbilisi: the moment a daily digest is due.
const NOW: &str = "2026-09-28T05:00:00Z";

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn config() -> PriceConfig {
    PriceConfig::daily(
        vec![AssetId::new("bitcoin"), AssetId::new("cow-protocol")],
        TimeZoneName::new("Asia/Tbilisi"),
    )
}

fn source() -> StubPrices {
    StubPrices::new()
        .priced("bitcoin", 84_753.0)
        .priced("cow-protocol", 0.160295)
}

fn due_digest() -> Digest {
    let mut digest = Digest::new(
        OWNER,
        Recurrence::Daily {
            at: jiff::civil::time(9, 0, 0, 0),
        },
        TimeZoneName::new("Asia/Tbilisi"),
        config().assets,
    );
    digest.next_fire_at = Some(ts(NOW));
    digest
}

async fn scheduler(
    source: StubPrices,
    notifier: RecordingDigestNotifier,
    digest: Option<Digest>,
) -> DigestScheduler<InMemoryDigestRepository, StubPrices, RecordingDigestNotifier, FixedClock> {
    let repository = InMemoryDigestRepository::new();
    if let Some(digest) = digest {
        repository.put(&digest).await.unwrap();
    }
    DigestScheduler::new(repository, source, notifier, FixedClock::at(NOW), config())
}

#[tokio::test]
async fn a_due_digest_is_read_and_sent() {
    let scheduler = scheduler(source(), RecordingDigestNotifier::new(), Some(due_digest())).await;
    let report = scheduler.tick().await.unwrap();

    assert_eq!(report.delivered, 1);
    let sent = scheduler.notifier.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, OWNER);
    assert_eq!(sent[0].1.quotes.len(), 2);
}

#[tokio::test]
async fn what_was_sent_is_cached_so_asking_afterwards_costs_nothing() {
    // The whole bargain: the scheduled run pays for the market read.
    let scheduler = scheduler(source(), RecordingDigestNotifier::new(), Some(due_digest())).await;
    scheduler.tick().await.unwrap();

    let stored = scheduler.repository.get(OWNER).await.unwrap().unwrap();
    assert_eq!(stored.cached.unwrap().quotes.len(), 2);
    assert_eq!(stored.last_sent_at, Some(ts(NOW)));
}

#[tokio::test]
async fn a_digest_that_has_been_sent_moves_to_the_next_day() {
    let scheduler = scheduler(source(), RecordingDigestNotifier::new(), Some(due_digest())).await;
    scheduler.tick().await.unwrap();

    let stored = scheduler.repository.get(OWNER).await.unwrap().unwrap();
    assert_eq!(stored.next_fire_at, Some(ts("2026-09-29T05:00:00Z")));
}

#[tokio::test]
async fn a_week_of_downtime_produces_one_message_rather_than_seven() {
    // The digest came due every morning while the process was down. Rescheduling from the
    // present, not from the moment missed, is what keeps it to one.
    let mut overdue = due_digest();
    overdue.next_fire_at = Some(ts("2026-09-21T05:00:00Z"));
    let scheduler = scheduler(source(), RecordingDigestNotifier::new(), Some(overdue)).await;

    let report = scheduler.tick().await.unwrap();
    assert_eq!(report.delivered, 1);
    assert_eq!(scheduler.notifier.sent().len(), 1);
    assert_eq!(scheduler.tick().await.unwrap().delivered, 0, "and no more");
}

#[tokio::test]
async fn nothing_is_sent_when_the_market_cannot_be_read() {
    // A digest of nothing is worse than a digest one tick late.
    let scheduler = scheduler(
        StubPrices::broken(PriceError::new("429 (rate limited)")),
        RecordingDigestNotifier::new(),
        Some(due_digest()),
    )
    .await;

    let report = scheduler.tick().await.unwrap();
    assert_eq!(report.delivered, 0);
    assert_eq!(report.retrying, 1);
    assert!(scheduler.notifier.sent().is_empty());
}

#[tokio::test]
async fn a_digest_left_unread_is_still_due_on_the_next_tick() {
    // A rate limit that lasts a minute must not cost the day's digest.
    let repository = std::sync::Arc::new(InMemoryDigestRepository::new());
    repository.put(&due_digest()).await.unwrap();
    let broken = DigestScheduler::new(
        std::sync::Arc::clone(&repository),
        StubPrices::broken(PriceError::new("429")),
        RecordingDigestNotifier::new(),
        FixedClock::at(NOW),
        config(),
    );
    broken.tick().await.unwrap();

    let working = DigestScheduler::new(
        std::sync::Arc::clone(&repository),
        source(),
        RecordingDigestNotifier::new(),
        FixedClock::at(NOW),
        config(),
    );
    assert_eq!(working.tick().await.unwrap().delivered, 1);
}

#[tokio::test]
async fn a_recipient_who_has_blocked_the_bot_has_the_digest_switched_off() {
    // Retrying forever means every tick paying for a delivery that cannot land.
    let scheduler = scheduler(
        source(),
        RecordingDigestNotifier::unreachable(),
        Some(due_digest()),
    )
    .await;
    let report = scheduler.tick().await.unwrap();

    assert_eq!(report.deactivated, 1);
    let stored = scheduler.repository.get(OWNER).await.unwrap().unwrap();
    assert!(!stored.is_active());
    assert!(stored.cached.is_some(), "what was read is kept");
}

#[tokio::test]
async fn a_digest_that_is_not_due_is_left_alone() {
    let mut later = due_digest();
    later.next_fire_at = Some(ts("2026-09-29T05:00:00Z"));
    let scheduler = scheduler(source(), RecordingDigestNotifier::new(), Some(later)).await;

    assert!(scheduler.tick().await.unwrap().is_quiet());
    assert_eq!(scheduler.source.calls(), 0, "and the market is not read");
}

#[tokio::test]
async fn a_tick_with_nothing_scheduled_is_quiet() {
    let scheduler = scheduler(source(), RecordingDigestNotifier::new(), None).await;
    assert!(scheduler.tick().await.unwrap().is_quiet());
}
