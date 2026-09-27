//! What the calendar does, against a calendar in memory.

use serana_domain::calendar::{CalendarError, CalendarEvent, Cancellation, EventId};
use serana_testkit::{FixedClock, InMemoryCalendar};

use super::*;

const TRAVEL: Duration = Duration::from_secs(30 * 60);

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn date(raw: &str) -> jiff::civil::Date {
    raw.parse().unwrap()
}

fn event(id: &str, summary: &str, starts_at: &str, ends_at: &str) -> CalendarEvent {
    CalendarEvent {
        id: EventId::new(id),
        summary: summary.into(),
        starts_at: ts(starts_at),
        ends_at: ts(ends_at),
        all_day: false,
        location: None,
        mine: true,
        recurring: false,
    }
}

/// Tbilisi is UTC+4 with no daylight saving, so a local hour is a fixed offset from the
/// timestamps these tests are written in.
fn service(calendar: InMemoryCalendar) -> CalendarService<InMemoryCalendar, FixedClock> {
    CalendarService::new(
        calendar,
        FixedClock::at("2026-09-25T06:00:00Z"),
        TimeZoneName::new("Asia/Tbilisi"),
        TRAVEL,
    )
}

#[tokio::test]
async fn upcoming_reaches_a_week_ahead_and_no_further() {
    let calendar = InMemoryCalendar::new()
        .with(event(
            "a",
            "dentist",
            "2026-09-26T10:00:00Z",
            "2026-09-26T11:00:00Z",
        ))
        .with(event(
            "b",
            "far off",
            "2026-10-20T10:00:00Z",
            "2026-10-20T11:00:00Z",
        ));
    let service = service(calendar);
    let found = service.upcoming(service.now()).await.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].summary, "dentist");
}

#[tokio::test]
async fn a_range_of_days_includes_the_last_one_to_its_end() {
    // An event at 23:30 on the closing day is on that day. Reading the range as
    // midnight-to-midnight would silently drop it.
    let calendar = InMemoryCalendar::new().with(event(
        "a",
        "late",
        "2026-09-27T19:30:00Z", // 23:30 local
        "2026-09-27T20:30:00Z",
    ));
    let (found, from, to) = service(calendar)
        .between(date("2026-09-26"), date("2026-09-27"))
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(from, ts("2026-09-25T20:00:00Z"), "midnight local");
    assert_eq!(
        to,
        ts("2026-09-27T20:00:00Z"),
        "midnight after the last day"
    );
}

#[tokio::test]
async fn a_range_that_ends_before_it_starts_is_refused_rather_than_returning_nothing() {
    // Nothing found and a nonsense question look identical to the person otherwise.
    assert!(matches!(
        service(InMemoryCalendar::new())
            .between(date("2026-09-27"), date("2026-09-20"))
            .await,
        Err(CalendarTurnError::Unparsable(_))
    ));
}

#[tokio::test]
async fn an_appointment_in_person_reserves_the_road_either_side() {
    let service = service(InMemoryCalendar::new());
    let (event, travel) = service
        .create(
            "dentist",
            date("2026-09-26"),
            jiff::civil::time(14, 0, 0, 0),
            Some(jiff::civil::time(15, 0, 0, 0)),
            Some("Chavchavadze 1".into()),
            true,
        )
        .await
        .unwrap();

    assert_eq!(travel, TRAVEL);
    // 14:00 local is 10:00Z; the block starts half an hour before and ends half an hour
    // after, so "am I free at 14:45?" answers correctly while the person is on the road.
    assert_eq!(event.starts_at, ts("2026-09-26T09:30:00Z"));
    assert_eq!(event.ends_at, ts("2026-09-26T11:30:00Z"));
    assert_eq!(event.location.as_deref(), Some("Chavchavadze 1"));
}

#[tokio::test]
async fn something_online_reserves_only_itself() {
    let service = service(InMemoryCalendar::new());
    let (event, travel) = service
        .create(
            "standup",
            date("2026-09-26"),
            jiff::civil::time(14, 0, 0, 0),
            Some(jiff::civil::time(14, 30, 0, 0)),
            None,
            false,
        )
        .await
        .unwrap();

    assert!(travel.is_zero());
    assert_eq!(event.starts_at, ts("2026-09-26T10:00:00Z"));
    assert_eq!(event.ends_at, ts("2026-09-26T10:30:00Z"));
}

#[tokio::test]
async fn an_appointment_with_no_end_given_lasts_an_hour() {
    // "dentist at 3" is the normal way to say it; the reply says what was booked, so an
    // hour that is wrong can be corrected in the next breath.
    let service = service(InMemoryCalendar::new());
    let (event, _) = service
        .create(
            "dentist",
            date("2026-09-26"),
            jiff::civil::time(15, 0, 0, 0),
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(event.starts_at, ts("2026-09-26T11:00:00Z"));
    assert_eq!(event.ends_at, ts("2026-09-26T12:00:00Z"));
}

#[tokio::test]
async fn an_appointment_that_ends_before_it_starts_is_refused() {
    let service = service(InMemoryCalendar::new());
    assert!(matches!(
        service
            .create(
                "dentist",
                date("2026-09-26"),
                jiff::civil::time(15, 0, 0, 0),
                Some(jiff::civil::time(14, 0, 0, 0)),
                None,
                false,
            )
            .await,
        Err(CalendarTurnError::Unparsable(_))
    ));
}

#[tokio::test]
async fn an_appointment_with_no_name_is_refused_rather_than_stored_blank() {
    let service = service(InMemoryCalendar::new());
    assert!(matches!(
        service
            .create(
                "  ",
                date("2026-09-26"),
                jiff::civil::time(15, 0, 0, 0),
                None,
                None,
                false,
            )
            .await,
        Err(CalendarTurnError::Unparsable(_))
    ));
}

#[tokio::test]
async fn cancelling_our_own_event_strikes_it_off() {
    let calendar = InMemoryCalendar::new().with(event(
        "a",
        "dentist",
        "2026-09-26T10:00:00Z",
        "2026-09-26T11:00:00Z",
    ));
    let service = service(calendar);
    let (event, how) = service.cancel(&EventId::new("a")).await.unwrap();

    assert_eq!(how, Cancellation::StrikeOff);
    assert_eq!(event.summary, "dentist");
}

#[tokio::test]
async fn cancelling_somebody_else_s_event_declines_it_and_leaves_it_there() {
    // Removing another person's event from their own invitation is not ours to do.
    let mut invited = event(
        "a",
        "review",
        "2026-09-26T10:00:00Z",
        "2026-09-26T11:00:00Z",
    );
    invited.mine = false;
    let service = service(InMemoryCalendar::new().with(invited));

    let (_, how) = service.cancel(&EventId::new("a")).await.unwrap();
    assert_eq!(how, Cancellation::Decline);
    assert_eq!(service.calendar.calls("decline"), vec![EventId::new("a")]);
    assert_eq!(service.calendar.events().len(), 1, "still on the calendar");
}

#[tokio::test]
async fn cancelling_one_occurrence_of_a_series_touches_only_that_occurrence() {
    // The person almost always means today's, not every Monday from now on. Expanded
    // occurrences carry their own ids, so striking one off is what reaches the provider.
    let mut weekly = event(
        "a_20260928",
        "standup",
        "2026-09-28T06:00:00Z",
        "2026-09-28T06:15:00Z",
    );
    weekly.recurring = true;
    let service = service(InMemoryCalendar::new().with(weekly));

    let (event, how) = service.cancel(&EventId::new("a_20260928")).await.unwrap();
    assert!(event.recurring, "so the reply can say which it was");
    assert_eq!(how, Cancellation::StrikeOff);
    assert_eq!(
        service.calendar.calls("strike_off"),
        vec![EventId::new("a_20260928")]
    );
}

#[tokio::test]
async fn deleting_reads_the_event_first_so_the_reply_can_name_what_went() {
    let calendar = InMemoryCalendar::new().with(event(
        "a",
        "dentist",
        "2026-09-26T10:00:00Z",
        "2026-09-26T11:00:00Z",
    ));
    let service = service(calendar);
    let gone = service.delete(&EventId::new("a")).await.unwrap();

    assert_eq!(gone.summary, "dentist");
    assert_eq!(service.calendar.calls("delete"), vec![EventId::new("a")]);
    assert!(service.calendar.events().is_empty());
}

#[tokio::test]
async fn an_id_the_model_invented_is_refused_before_anything_is_sent() {
    let service = service(InMemoryCalendar::new());
    for outcome in [
        service.cancel(&EventId::new("nope")).await,
        service
            .delete(&EventId::new("nope"))
            .await
            .map(|e| (e, Cancellation::StrikeOff)),
    ] {
        assert!(matches!(outcome, Err(CalendarTurnError::NotFound(_))));
    }
    assert!(service.calendar.calls("delete").is_empty());
    assert!(service.calendar.calls("strike_off").is_empty());
}

#[tokio::test]
async fn a_calendar_that_is_down_says_so_rather_than_looking_empty() {
    let service = service(InMemoryCalendar::broken(CalendarError::Unavailable(
        "503".into(),
    )));
    assert!(matches!(
        service.upcoming(service.now()).await,
        Err(CalendarTurnError::Calendar(CalendarError::Unavailable(_)))
    ));
}
