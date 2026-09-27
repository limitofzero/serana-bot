use serana_domain::prices::{AssetId, AssetMatch};
use serana_domain::reminder::{Recurrence, UserId};

use super::*;

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn zone() -> TimeZoneName {
    TimeZoneName::new("Asia/Tbilisi")
}

fn quote(symbol: &str, usd: f64, change: Option<f64>) -> Quote {
    Quote {
        asset: AssetId::new(symbol.to_lowercase()),
        symbol: symbol.into(),
        usd,
        change_24h: change,
    }
}

fn market(at: &str) -> Snapshot {
    Snapshot::new(
        vec![
            quote("BTC", 84_753.0, Some(0.90091)),
            quote("ETH", 2_690.38, Some(0.38441)),
            quote("COW", 0.160295, Some(4.57522)),
        ],
        ts(at),
        "CoinGecko",
    )
}

fn standing() -> Digest {
    let mut digest = Digest::new(
        UserId::new(42),
        Recurrence::Daily {
            at: jiff::civil::time(9, 0, 0, 0),
        },
        zone(),
        vec![AssetId::new("bitcoin")],
    );
    digest.next_fire_at = Some(ts("2026-09-29T05:00:00Z"));
    digest
}

#[test]
fn each_price_is_shown_to_as_many_places_as_it_is_worth_reading() {
    // The same message holds a five-figure number and one that lives in its fourth decimal.
    // A fixed precision is wrong for one of them whichever you pick.
    let reply = digest(
        &market("2026-09-28T05:00:00Z"),
        ts("2026-09-28T05:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("$84,753"), "{reply}");
    assert!(reply.contains("$2,690.38"), "{reply}");
    assert!(reply.contains("$0.1603"), "{reply}");
}

#[test]
fn a_five_figure_price_is_grouped_so_it_reads_at_a_glance() {
    assert_eq!(usd(84_753.0), "$84,753");
    assert_eq!(usd(1_234_567.0), "$1,234,567");
    assert_eq!(usd(999.5), "$999.50");
}

#[test]
fn a_price_in_its_sixth_decimal_place_is_not_rounded_to_nothing() {
    // "$0.00" for a real price is the worst rendering there is.
    assert_eq!(usd(0.000123), "$0.000123");
}

#[test]
fn the_24_hour_move_carries_its_direction() {
    let reply = digest(
        &market("2026-09-28T05:00:00Z"),
        ts("2026-09-28T05:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("▲ 0.90%"), "{reply}");
    assert!(reply.contains("▲ 4.58%"), "{reply}");
}

#[test]
fn a_fall_is_shown_as_one_rather_than_as_a_negative_rise() {
    let snapshot = Snapshot::new(
        vec![quote("BTC", 84_753.0, Some(-2.5))],
        ts("2026-09-28T05:00:00Z"),
        "CoinGecko",
    );
    let reply = digest(&snapshot, ts("2026-09-28T05:00:00Z"), &zone());
    assert!(reply.contains("▼ 2.50%"), "{reply}");
    assert!(!reply.contains("-2.50"), "{reply}");
}

#[test]
fn a_source_that_does_not_report_the_move_renders_silence_rather_than_zero() {
    // A flat market and an unknown one must not look the same.
    let snapshot = Snapshot::new(
        vec![quote("BTC", 84_753.0, None)],
        ts("2026-09-28T05:00:00Z"),
        "DefiLlama",
    );
    let reply = digest(&snapshot, ts("2026-09-28T05:00:00Z"), &zone());
    assert!(!reply.contains('%'), "{reply}");
    assert!(!reply.contains("0.00"), "{reply}");
}

#[test]
fn prices_say_how_old_they_are() {
    // A price without its moment is a number nobody can act on.
    let snapshot = market("2026-09-28T05:00:00Z");
    for (now, expected) in [
        ("2026-09-28T05:00:00Z", "just now"),
        ("2026-09-28T05:30:00Z", "30 minutes ago"),
        ("2026-09-28T09:00:00Z", "4 hours ago"),
    ] {
        let reply = digest(&snapshot, ts(now), &zone());
        assert!(reply.contains(expected), "{now}: {reply}");
    }
}

#[test]
fn prices_from_days_ago_are_dated_rather_than_counted_in_hours() {
    let reply = digest(
        &market("2026-09-25T05:00:00Z"),
        ts("2026-09-28T05:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("on 25 September at 09:00"), "{reply}");
}

#[test]
fn an_answer_to_a_question_says_when_the_next_digest_lands() {
    let reply = outcome(
        &PriceOutcome::Showed {
            digest: standing(),
            snapshot: market("2026-09-28T05:00:00Z"),
            read_now: true,
        },
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(reply.contains("Next: 29 September at 09:00"), "{reply}");
    assert!(reply.contains("every day at 09:00"), "{reply}");
}

#[test]
fn a_digest_that_is_switched_off_says_how_to_turn_it_back_on() {
    let mut off = standing();
    off.next_fire_at = None;
    let reply = outcome(
        &PriceOutcome::Showed {
            digest: off,
            snapshot: market("2026-09-28T05:00:00Z"),
            read_now: false,
        },
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(reply.contains("Not arriving"), "{reply}");
}

#[test]
fn moving_the_digest_confirms_where_it_moved_to() {
    let mut moved = standing();
    moved.recurrence = Recurrence::Daily {
        at: jiff::civil::time(7, 30, 0, 0),
    };
    moved.next_fire_at = Some(ts("2026-09-29T03:30:00Z"));
    let reply = rescheduled(&moved);
    assert!(reply.contains("29 September at 07:30"), "{reply}");
    assert!(reply.contains("every day at 07:30"), "{reply}");
}

#[test]
fn an_empty_market_says_so_rather_than_showing_an_empty_list() {
    let empty = Snapshot::new(Vec::new(), ts("2026-09-28T05:00:00Z"), "CoinGecko");
    assert_eq!(
        digest(&empty, ts("2026-09-28T05:00:00Z"), &zone()),
        NO_MARKET
    );
}

#[test]
fn the_ticker_is_echoed_exactly_as_the_source_gave_it() {
    let snapshot = Snapshot::new(
        vec![quote("wSTETH", 3_100.0, None)],
        ts("2026-09-28T05:00:00Z"),
        "CoinGecko",
    );
    assert!(
        digest(&snapshot, ts("2026-09-28T05:00:00Z"), &zone()).contains("wSTETH"),
        "not upper-cased or otherwise tidied"
    );
}

#[test]
fn a_pushed_digest_reads_as_current_because_it_was_read_to_send() {
    // The scheduler reads the market and sends in one breath, so there is no age to report
    // and no zone to report it in.
    let reply = pushed(&market("2026-09-28T05:00:00Z"));
    assert!(reply.contains("just now"), "{reply}");
    assert!(reply.contains("$84,753"), "{reply}");
}

#[test]
fn prose_from_the_model_is_shown_verbatim() {
    assert_eq!(
        outcome(
            &PriceOutcome::Said("What time of day?".into()),
            ts("2026-09-28T05:00:00Z")
        ),
        "What time of day?"
    );
}

fn found(id: &str, name: &str, symbol: &str) -> AssetMatch {
    AssetMatch {
        id: AssetId::new(id),
        name: name.into(),
        symbol: symbol.into(),
        rank: Some(1),
    }
}

fn edit() -> WatchlistEdit {
    WatchlistEdit {
        digest: standing(),
        added: Vec::new(),
        already: Vec::new(),
        removed: Vec::new(),
        missing: Vec::new(),
    }
}

#[test]
fn an_addition_names_what_the_lookup_found_so_it_can_be_checked() {
    let mut change = edit();
    change.added = vec![found("uniswap", "Uniswap", "UNI")];
    let reply = edited(
        &change,
        Some(&market("2026-09-28T05:00:00Z")),
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(reply.contains("Added UNI (Uniswap)"), "{reply}");
}

#[test]
fn a_name_that_only_repeats_the_ticker_is_not_shown_twice() {
    let mut change = edit();
    change.added = vec![found("1inch", "1INCH", "1INCH")];
    let reply = edited(
        &change,
        Some(&market("2026-09-28T05:00:00Z")),
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(reply.contains("Added 1INCH"), "{reply}");
    assert!(!reply.contains("1INCH (1INCH)"), "{reply}");
}

#[test]
fn a_change_is_followed_by_the_digest_as_it_now_stands() {
    // "What am I watching now?" is the question, not "did it work".
    let mut change = edit();
    change.removed = vec!["ETH".into()];
    let reply = edited(
        &change,
        Some(&market("2026-09-28T05:00:00Z")),
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(reply.contains("Removed ETH"), "{reply}");
    assert!(reply.contains("💰 Prices"), "{reply}");
    assert!(reply.contains("Next:"), "{reply}");
}

#[test]
fn every_name_asked_about_is_accounted_for() {
    let mut change = edit();
    change.added = vec![found("uniswap", "Uniswap", "UNI")];
    change.already = vec![found("cow-protocol", "CoW Protocol", "COW")];
    change.missing = vec!["notarealcoin".into()];
    let reply = edited(
        &change,
        Some(&market("2026-09-28T05:00:00Z")),
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(reply.contains("UNI"), "{reply}");
    assert!(reply.contains("Already watching COW"), "{reply}");
    assert!(reply.contains("Could not find notarealcoin"), "{reply}");
}

#[test]
fn nothing_changing_shows_no_digest() {
    // Only "could not find" to say; a full price list under it would bury that.
    let mut change = edit();
    change.missing = vec!["notarealcoin".into()];
    let reply = edited(
        &change,
        Some(&market("2026-09-28T05:00:00Z")),
        ts("2026-09-28T05:00:00Z"),
    );
    assert!(!reply.contains("💰"), "{reply}");
}

#[test]
fn an_addition_the_market_could_not_price_yet_still_confirms_it() {
    let mut change = edit();
    change.added = vec![found("uniswap", "Uniswap", "UNI")];
    let reply = edited(&change, None, ts("2026-09-28T05:00:00Z"));
    assert!(reply.contains("Added UNI"), "{reply}");
    assert!(reply.contains("next /prices"), "{reply}");
}
