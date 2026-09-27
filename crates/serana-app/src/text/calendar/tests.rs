use serana_domain::calendar::EventId;

use super::*;

fn ts(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn zone() -> TimeZoneName {
    TimeZoneName::new("Asia/Tbilisi")
}

fn event(summary: &str, starts_at: &str, ends_at: &str) -> CalendarEvent {
    CalendarEvent {
        id: EventId::new("ev1"),
        summary: summary.into(),
        starts_at: ts(starts_at),
        ends_at: ts(ends_at),
        all_day: false,
        location: None,
        mine: true,
        recurring: false,
    }
}

#[test]
fn times_are_shown_in_the_person_s_own_zone() {
    // 10:00Z is 14:00 in Tbilisi, and local is the only clock they think in.
    let reply = created(
        &event("dentist", "2026-09-26T10:00:00Z", "2026-09-26T11:00:00Z"),
        std::time::Duration::ZERO,
        &zone(),
    );
    assert!(reply.contains("14:00–15:00"), "{reply}");
    assert!(reply.contains("Saturday 26 September"), "{reply}");
}

#[test]
fn a_booking_with_travel_shows_both_the_appointment_and_the_block() {
    // The block is wider than the appointment on purpose. Unexplained, it looks like a bug
    // the next time they open the calendar.
    let blocked = event("dentist", "2026-09-26T09:30:00Z", "2026-09-26T11:30:00Z");
    let reply = created(&blocked, std::time::Duration::from_secs(30 * 60), &zone());
    assert!(reply.contains("14:00–15:00"), "the appointment: {reply}");
    assert!(reply.contains("13:30–15:30"), "the block: {reply}");
    assert!(reply.contains("30 min either side"), "{reply}");
}

#[test]
fn a_booking_with_no_travel_says_nothing_about_the_road() {
    let reply = created(
        &event("standup", "2026-09-26T10:00:00Z", "2026-09-26T10:15:00Z"),
        std::time::Duration::ZERO,
        &zone(),
    );
    assert!(!reply.contains("🚗"), "{reply}");
}

#[test]
fn the_title_is_echoed_back_exactly_as_it_was_written() {
    // Whatever language it is in, and whatever is in it.
    let reply = created(
        &event(
            "Оформить invoice 📄",
            "2026-09-26T10:00:00Z",
            "2026-09-26T11:00:00Z",
        ),
        std::time::Duration::ZERO,
        &zone(),
    );
    assert!(reply.contains("Оформить invoice 📄"), "{reply}");
}

#[test]
fn a_listing_groups_by_day_and_names_the_span_it_searched() {
    let events = vec![
        event("standup", "2026-09-26T06:00:00Z", "2026-09-26T06:15:00Z"),
        event("dentist", "2026-09-26T10:00:00Z", "2026-09-26T11:00:00Z"),
        event("dinner", "2026-09-27T16:00:00Z", "2026-09-27T18:00:00Z"),
    ];
    let reply = listed(
        &events,
        ts("2026-09-25T20:00:00Z"),
        ts("2026-09-27T20:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("26 September – 27 September"), "{reply}");
    assert_eq!(reply.matches("Saturday 26 September").count(), 1, "{reply}");
    assert_eq!(reply.matches("Sunday 27 September").count(), 1, "{reply}");
    assert!(reply.contains("10:00–10:15  standup"), "{reply}");
}

#[test]
fn a_single_day_is_named_rather_than_shown_as_a_range_to_itself() {
    let reply = listed(
        &[event(
            "dentist",
            "2026-09-26T10:00:00Z",
            "2026-09-26T11:00:00Z",
        )],
        ts("2026-09-25T20:00:00Z"),
        ts("2026-09-26T20:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("📅 Saturday 26 September"), "{reply}");
    assert!(!reply.contains(" – "), "{reply}");
}

#[test]
fn an_empty_day_says_so_against_the_span_that_was_asked_about() {
    // "Nothing on" alone is unreadable: nothing on *when*?
    let reply = listed(
        &[],
        ts("2026-09-25T20:00:00Z"),
        ts("2026-09-26T20:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("Saturday 26 September"), "{reply}");
    assert!(reply.contains("Nothing on"), "{reply}");
}

#[test]
fn an_all_day_event_is_not_given_invented_hours() {
    let mut all_day = event(
        "public holiday",
        "2026-09-25T20:00:00Z",
        "2026-09-26T20:00:00Z",
    );
    all_day.all_day = true;
    let reply = listed(
        std::slice::from_ref(&all_day),
        ts("2026-09-25T20:00:00Z"),
        ts("2026-09-26T20:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("all day"), "{reply}");
    assert!(!reply.contains("00:00"), "{reply}");
}

#[test]
fn a_place_is_shown_when_there_is_one() {
    let mut somewhere = event("dentist", "2026-09-26T10:00:00Z", "2026-09-26T11:00:00Z");
    somewhere.location = Some("Chavchavadze 1".into());
    let reply = created(&somewhere, std::time::Duration::ZERO, &zone());
    assert!(reply.contains("📍 Chavchavadze 1"), "{reply}");
}

#[test]
fn declining_says_the_event_is_still_there() {
    // Otherwise they go looking for it, find it, and think the bot did nothing.
    let mut theirs = event("review", "2026-09-26T10:00:00Z", "2026-09-26T11:00:00Z");
    theirs.mine = false;
    let reply = cancelled(&theirs, Cancellation::Decline, &zone());
    assert!(reply.contains("stays on your calendar"), "{reply}");
    assert!(reply.contains("review"), "{reply}");
}

#[test]
fn cancelling_one_occurrence_says_the_series_stands() {
    let mut weekly = event("standup", "2026-09-28T06:00:00Z", "2026-09-28T06:15:00Z");
    weekly.recurring = true;
    let reply = cancelled(&weekly, Cancellation::StrikeOff, &zone());
    assert!(reply.contains("the rest of the series stands"), "{reply}");
}

#[test]
fn deleting_says_it_is_gone_rather_than_merely_off() {
    let reply = deleted(
        &event("dentist", "2026-09-26T10:00:00Z", "2026-09-26T11:00:00Z"),
        &zone(),
    );
    assert!(reply.contains("Deleted"), "{reply}");
}

#[test]
fn prose_from_the_model_is_shown_verbatim() {
    // It is usually a question back, or the confirmation asked for before a delete.
    let reply = outcome(
        &CalendarOutcome::Said("Delete the dentist on Friday? This cannot be undone.".into()),
        &zone(),
    );
    assert_eq!(
        reply,
        "Delete the dentist on Friday? This cannot be undone."
    );
}
