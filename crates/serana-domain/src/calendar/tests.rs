//! Behaviour of calendar events and the drafts that become them.

use std::time::Duration;

use super::*;

fn at(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

fn event(starts: &str, ends: &str) -> CalendarEvent {
    CalendarEvent {
        id: EventId::new("e1"),
        summary: "stand-up".into(),
        starts_at: at(starts),
        ends_at: at(ends),
        all_day: false,
        location: None,
        mine: true,
        recurring: false,
    }
}

fn draft(starts: &str, ends: &str, travel: Duration) -> NewEvent {
    NewEvent {
        summary: "dentist".into(),
        starts_at: at(starts),
        ends_at: at(ends),
        travel,
        location: None,
    }
}

#[test]
fn overlap_is_half_open_at_both_ends() {
    let e = event("2026-09-24T09:00:00Z", "2026-09-24T10:00:00Z");
    // Touching but not overlapping, in both directions: back-to-back meetings do not clash.
    assert!(!e.overlaps(at("2026-09-24T08:00:00Z"), at("2026-09-24T09:00:00Z")));
    assert!(!e.overlaps(at("2026-09-24T10:00:00Z"), at("2026-09-24T11:00:00Z")));
    // Straddling, contained, and containing.
    assert!(e.overlaps(at("2026-09-24T08:30:00Z"), at("2026-09-24T09:30:00Z")));
    assert!(e.overlaps(at("2026-09-24T09:15:00Z"), at("2026-09-24T09:30:00Z")));
    assert!(e.overlaps(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z")));
}

#[test]
fn cancelling_my_own_event_strikes_it_off_and_someone_elses_declines() {
    // The distinction that decides whether the event vanishes from the day or stays put
    // marked as not attending. Getting it backwards either fails silently or removes
    // something that was never ours to remove.
    let mut e = event("2026-09-24T09:00:00Z", "2026-09-24T10:00:00Z");
    assert_eq!(e.cancellation(), Cancellation::StrikeOff);

    e.mine = false;
    assert_eq!(e.cancellation(), Cancellation::Decline);
}

#[test]
fn ownership_decides_cancellation_regardless_of_recurrence() {
    // A repeating event of someone else's is still declined, not struck off.
    let mut e = event("2026-09-24T09:00:00Z", "2026-09-24T10:00:00Z");
    e.recurring = true;
    e.mine = false;
    assert_eq!(e.cancellation(), Cancellation::Decline);
}

#[test]
fn travel_is_reserved_at_both_ends_of_the_appointment() {
    // The whole point: "am I free at 14:45?" must answer no while the person is driving.
    let d = draft(
        "2026-09-24T15:00:00Z",
        "2026-09-24T15:45:00Z",
        Duration::from_secs(30 * 60),
    );
    assert_eq!(
        d.blocked(),
        (at("2026-09-24T14:30:00Z"), at("2026-09-24T16:15:00Z"))
    );
    assert!(d.has_travel());
}

#[test]
fn something_online_reserves_only_itself() {
    let d = draft(
        "2026-09-24T15:00:00Z",
        "2026-09-24T15:45:00Z",
        Duration::ZERO,
    );
    assert_eq!(
        d.blocked(),
        (at("2026-09-24T15:00:00Z"), at("2026-09-24T15:45:00Z"))
    );
    assert!(!d.has_travel());
}

#[test]
fn an_absurd_travel_time_reserves_the_appointment_rather_than_failing() {
    // The argument comes from a model, so it can be nonsense. Reserving the appointment
    // alone is the reading that loses least.
    let d = draft(
        "2026-09-24T15:00:00Z",
        "2026-09-24T15:45:00Z",
        Duration::from_secs(u64::MAX),
    );
    assert_eq!(
        d.blocked(),
        (at("2026-09-24T15:00:00Z"), at("2026-09-24T15:45:00Z"))
    );
}

#[test]
fn an_event_round_trips_through_serde() {
    // It rides into the model as a tool result, so it has to serialise faithfully.
    let mut e = event("2026-09-24T09:00:00Z", "2026-09-24T10:00:00Z");
    e.location = Some("Rustaveli 12".into());
    e.recurring = true;
    let json = serde_json::to_string(&e).unwrap();
    assert_eq!(serde_json::from_str::<CalendarEvent>(&json).unwrap(), e);
}
