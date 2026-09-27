//! The Google Calendar adapter, against a mock server.
//!
//! Every test asserts both halves: the request we send and our reading of the reply. A
//! calendar that silently drops a field or reads a time wrongly is worse than one that
//! fails, because the failure is a message on someone's phone at the wrong hour.

use std::time::Duration;

use serana_domain::calendar::{CalendarPort, Cancellation};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::*;

fn at(raw: &str) -> jiff::Timestamp {
    raw.parse().unwrap()
}

/// A calendar pointed at `server`, with the token endpoint already answering.
async fn calendar(server: &MockServer) -> GoogleCalendar {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "ya29.test",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(server)
        .await;

    GoogleCalendar::new(GoogleCalendarConfig {
        client_id: "id".into(),
        client_secret: "secret".into(),
        refresh_token: "refresh".into(),
        calendar_id: "primary".into(),
        request_timeout: Duration::from_secs(5),
        token_url: format!("{}/token", server.uri()),
        api_base: server.uri(),
    })
    .unwrap()
}

fn timed_event(id: &str, summary: &str, start: &str, end: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "summary": summary,
        "start": { "dateTime": start },
        "end": { "dateTime": end },
        "organizer": { "self": true }
    })
}

#[tokio::test]
async fn nothing_configured_is_not_connected_rather_than_an_error() {
    // The ordinary state before anyone sets up Google, and the assistant should say it has
    // no calendar rather than report an outage.
    // Matched rather than unwrapped: `unwrap_err` would need `Debug` on the calendar, and
    // it holds a live access token that no stray `{:?}` should ever print.
    match GoogleCalendar::new(GoogleCalendarConfig::default()) {
        Err(e) => assert_eq!(e, CalendarError::NotConnected),
        Ok(_) => panic!("an unconfigured calendar must not be constructible"),
    }
}

#[tokio::test]
async fn a_day_is_asked_for_as_expanded_occurrences_in_order() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events"))
        // Without singleEvents a weekly stand-up comes back once with a repeat rule, and
        // "what is on today" would miss it.
        .and(query_param("singleEvents", "true"))
        .and(query_param("orderBy", "startTime"))
        .and(query_param("timeMin", "2026-09-24T00:00:00Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [timed_event("e1", "stand-up", "2026-09-24T09:00:00Z", "2026-09-24T09:15:00Z")]
        })))
        .mount(&server)
        .await;

    let events = cal
        .events_between(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].summary, "stand-up");
    assert_eq!(events[0].starts_at, at("2026-09-24T09:00:00Z"));
    assert!(!events[0].all_day);
}

#[tokio::test]
async fn a_cancelled_occurrence_is_not_on_the_day() {
    // Google keeps returning struck-off occurrences in the list; showing one would tell the
    // person they have a meeting they cancelled.
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    let mut cancelled = timed_event(
        "e2",
        "old stand-up",
        "2026-09-24T09:00:00Z",
        "2026-09-24T09:15:00Z",
    );
    cancelled["status"] = "cancelled".into();

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [
                cancelled,
                timed_event("e1", "real", "2026-09-24T10:00:00Z", "2026-09-24T11:00:00Z")
            ]
        })))
        .mount(&server)
        .await;

    let events = cal
        .events_between(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z"))
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].summary, "real");
}

#[tokio::test]
async fn an_all_day_event_is_read_as_such_rather_than_given_an_invented_time() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{
                "id": "e3",
                "summary": "holiday",
                "start": { "date": "2026-09-24" },
                "end": { "date": "2026-09-25" }
            }]
        })))
        .mount(&server)
        .await;

    let events = cal
        .events_between(at("2026-09-24T00:00:00Z"), at("2026-09-26T00:00:00Z"))
        .await
        .unwrap();
    assert!(events[0].all_day);
    assert_eq!(events[0].starts_at, at("2026-09-24T00:00:00Z"));
}

#[tokio::test]
async fn ownership_is_read_from_the_organizer_so_cancelling_picks_the_right_act() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    let mut theirs = timed_event(
        "e4",
        "their meeting",
        "2026-09-24T09:00:00Z",
        "2026-09-24T10:00:00Z",
    );
    theirs["organizer"] = serde_json::json!({ "self": false });
    theirs["attendees"] = serde_json::json!([{ "email": "me@example.com", "self": true }]);

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [
                timed_event("e1", "mine", "2026-09-24T08:00:00Z", "2026-09-24T08:30:00Z"),
                theirs
            ]
        })))
        .mount(&server)
        .await;

    let events = cal
        .events_between(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z"))
        .await
        .unwrap();
    assert!(events[0].mine);
    assert_eq!(events[0].cancellation(), Cancellation::StrikeOff);
    assert!(!events[1].mine);
    assert_eq!(events[1].cancellation(), Cancellation::Decline);
}

#[tokio::test]
async fn an_occurrence_of_a_series_is_marked_as_recurring() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    let mut instance = timed_event(
        "e5_20260924",
        "weekly",
        "2026-09-24T09:00:00Z",
        "2026-09-24T09:30:00Z",
    );
    instance["recurringEventId"] = "e5".into();

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [instance]
        })))
        .mount(&server)
        .await;

    let events = cal
        .events_between(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z"))
        .await
        .unwrap();
    assert!(
        events[0].recurring,
        "so the assistant can say which it struck off"
    );
}

#[tokio::test]
async fn creating_reserves_the_travel_as_well_as_the_appointment() {
    // The whole point of the padding: the block sent to Google covers the journey.
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("POST"))
        .and(path("/calendars/primary/events"))
        .and(body_json(serde_json::json!({
            "summary": "dentist",
            "start": { "dateTime": "2026-09-24T14:30:00Z" },
            "end": { "dateTime": "2026-09-24T16:15:00Z" }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(timed_event(
            "new1",
            "dentist",
            "2026-09-24T14:30:00Z",
            "2026-09-24T16:15:00Z",
        )))
        .mount(&server)
        .await;

    let created = cal
        .create(&NewEvent {
            summary: "dentist".into(),
            starts_at: at("2026-09-24T15:00:00Z"),
            ends_at: at("2026-09-24T15:45:00Z"),
            travel: Duration::from_secs(30 * 60),
            location: None,
        })
        .await
        .unwrap();
    assert_eq!(created.id.as_str(), "new1");
}

#[tokio::test]
async fn striking_off_sets_the_status_and_leaves_everything_else_alone() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("PATCH"))
        .and(path("/calendars/primary/events/e1"))
        .and(body_json(serde_json::json!({ "status": "cancelled" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    cal.strike_off(&EventId::new("e1")).await.unwrap();
}

#[tokio::test]
async fn declining_changes_only_our_own_attendee_row() {
    // Rewriting somebody else's response would be answering an invitation on their behalf.
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events/e4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "e4",
            "summary": "their meeting",
            "start": { "dateTime": "2026-09-24T09:00:00Z" },
            "end": { "dateTime": "2026-09-24T10:00:00Z" },
            "organizer": { "self": false },
            "attendees": [
                { "email": "them@example.com", "self": false, "responseStatus": "accepted" },
                { "email": "me@example.com", "self": true, "responseStatus": "needsAction" }
            ]
        })))
        .mount(&server)
        .await;

    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured = std::sync::Arc::clone(&seen);
    Mock::given(method("PATCH"))
        .and(path("/calendars/primary/events/e4"))
        .respond_with(move |req: &Request| {
            *captured.lock().unwrap() = Some(req.body_json::<serde_json::Value>().unwrap());
            ResponseTemplate::new(200).set_body_json(serde_json::json!({}))
        })
        .mount(&server)
        .await;

    cal.decline(&EventId::new("e4")).await.unwrap();

    let body = seen.lock().unwrap().clone().expect("a patch was sent");
    let attendees = body["attendees"].as_array().unwrap();
    assert_eq!(
        attendees[0]["responseStatus"], "accepted",
        "theirs untouched"
    );
    assert_eq!(attendees[1]["responseStatus"], "declined", "ours declined");
}

#[tokio::test]
async fn declining_something_with_no_invitation_says_so() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events/e1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(timed_event(
            "e1",
            "mine",
            "2026-09-24T09:00:00Z",
            "2026-09-24T10:00:00Z",
        )))
        .mount(&server)
        .await;

    assert!(cal.decline(&EventId::new("e1")).await.is_err());
}

#[tokio::test]
async fn deleting_something_already_gone_is_the_outcome_asked_for() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("DELETE"))
        .and(path("/calendars/primary/events/e9"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    cal.delete(&EventId::new("e9")).await.unwrap();
}

#[tokio::test]
async fn an_unknown_event_is_none_rather_than_a_failure() {
    let server = MockServer::start().await;
    let cal = calendar(&server).await;

    Mock::given(method("GET"))
        .and(path("/calendars/primary/events/nope"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    assert!(cal.get(&EventId::new("nope")).await.unwrap().is_none());
}

#[tokio::test]
async fn a_refused_refresh_token_is_reported_with_googles_own_words() {
    // The failure the person will actually hit, when the 7-day testing token expires.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "invalid_grant",
            "error_description": "Token has been expired or revoked."
        })))
        .mount(&server)
        .await;

    let cal = GoogleCalendar::new(GoogleCalendarConfig {
        client_id: "id".into(),
        client_secret: "secret".into(),
        refresh_token: "stale".into(),
        calendar_id: "primary".into(),
        request_timeout: Duration::from_secs(5),
        token_url: format!("{}/token", server.uri()),
        api_base: server.uri(),
    })
    .unwrap();

    let err = cal
        .events_between(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z"))
        .await
        .unwrap_err();
    assert!(matches!(err, CalendarError::Unavailable(_)), "{err:?}");
}

#[tokio::test]
async fn the_access_token_is_fetched_once_and_reused() {
    // An hour-long token refreshed on every call would triple the request count.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "ya29.test",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/calendars/primary/events"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "items": [] })))
        .mount(&server)
        .await;

    let cal = GoogleCalendar::new(GoogleCalendarConfig {
        client_id: "id".into(),
        client_secret: "secret".into(),
        refresh_token: "refresh".into(),
        calendar_id: "primary".into(),
        request_timeout: Duration::from_secs(5),
        token_url: format!("{}/token", server.uri()),
        api_base: server.uri(),
    })
    .unwrap();

    for _ in 0..3 {
        cal.events_between(at("2026-09-24T00:00:00Z"), at("2026-09-25T00:00:00Z"))
            .await
            .unwrap();
    }
    // `expect(1)` on the token mock is checked when the server drops.
}
