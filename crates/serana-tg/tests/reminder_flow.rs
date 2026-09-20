//! End-to-end: a reminder request typed into Telegram, through a real model wire format and
//! a real database, out to a delivered message.
//!
//! Everything below the Telegram transport is genuine — the OpenAI adapter talking
//! chat-completions to a mock HTTP server, SQLite on disk, the real service and scheduler.
//! Only two things are substituted: the clock, so a month can pass in a microsecond, and the
//! notifier, because asserting on delivery is the whole point and Telegram's own HTTP is not
//! what these tests are about.

use std::sync::Arc;
use std::time::Duration;

use serana_adapters::{OpenAiConfig, OpenAiProvider, RandomIds, SqliteReminderRepository};
use serana_domain::reminder::{Recurrence, ReminderRepository, TimeZoneName, UserId};
use serana_services::{ReminderConfig, ReminderService, SchedulerService};
use serana_testkit::{FixedClock, RecordingNotifier};
use serana_tg::commands::{Command, respond};
use tempfile::TempDir;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const OWNER: UserId = UserId::new(42);
/// 10:00 in Tbilisi.
const CREATED_AT: &str = "2026-03-10T06:00:00Z";

type Service =
    ReminderService<SqliteReminderRepository, OpenAiProvider, Arc<FixedClock>, RandomIds>;

struct Harness {
    /// Held so the database file outlives the test.
    _dir: TempDir,
    database: std::path::PathBuf,
    repository: SqliteReminderRepository,
    service: Service,
    clock: Arc<FixedClock>,
}

/// A model that answers every request by asking for one monthly reminder on the 20th.
async fn model_extracting(arguments: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "schedule_reminder",
                            "arguments": arguments.to_string()
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 320, "completion_tokens": 24}
        })))
        .mount(&server)
        .await;
    server
}

fn invoice_schedule() -> serde_json::Value {
    serde_json::json!({
        "kind": "monthly",
        "time": "10:00",
        "day_of_month": 20,
        "text": "оформить invoice"
    })
}

async fn harness(server: &MockServer) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("serana.db");
    let repository = SqliteReminderRepository::open(&database).await.unwrap();
    let clock = Arc::new(FixedClock::at(CREATED_AT));

    let provider = OpenAiProvider::new(OpenAiConfig {
        base_url: server.uri(),
        api_key: "test-key".into(),
        request_timeout: Duration::from_secs(5),
        max_retries: 0,
        retry_min_delay: Duration::from_millis(1),
    })
    .unwrap();

    let service = ReminderService::new(
        repository.clone(),
        provider,
        Arc::clone(&clock),
        RandomIds,
        ReminderConfig {
            model: "gpt-5-mini".into(),
            default_timezone: TimeZoneName::new("Asia/Tbilisi"),
        },
    );

    Harness {
        _dir: dir,
        database,
        repository,
        service,
        clock,
    }
}

/// Pull the reminder id out of what the user was shown — the same way they would.
fn id_from(reply: &str) -> String {
    reply
        .lines()
        .find_map(|line| line.trim().strip_prefix("id: "))
        .unwrap_or_else(|| panic!("no id in reply:\n{reply}"))
        .trim()
        .to_owned()
}

#[tokio::test]
async fn a_request_becomes_a_stored_reminder_that_later_arrives() {
    let server = model_extracting(invoice_schedule()).await;
    let h = harness(&server).await;

    // 1. The user types the command from the brief.
    let reply = respond(
        &h.service,
        OWNER,
        &Command::Reminder("каждый месяц 20 число - писать мне что надо оформить invoice".into()),
    )
    .await;
    assert!(reply.contains("каждое 20 число в 10:00"), "{reply}");
    assert!(reply.contains("20 марта 2026 в 10:00"), "{reply}");
    let id = id_from(&reply);

    // 2. It is genuinely on disk, not just in a cache: reopen the file and look.
    let reopened = SqliteReminderRepository::open(&h.database).await.unwrap();
    let stored = reopened.list_for_owner(OWNER).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].text, "оформить invoice");
    assert_eq!(
        stored[0].recurrence,
        Recurrence::Monthly {
            day: 20,
            at: jiff::civil::time(10, 0, 0, 0)
        }
    );
    assert_eq!(stored[0].id.as_str(), id);

    // 3. The 20th arrives and the scheduler delivers it.
    let notifier = Arc::new(RecordingNotifier::new());
    let scheduler = SchedulerService::new(
        h.repository.clone(),
        Arc::clone(&notifier),
        Arc::clone(&h.clock),
    );
    assert!(
        scheduler.tick().await.unwrap().is_quiet(),
        "nothing is due on the 10th"
    );

    h.clock.set("2026-03-20T06:00:00Z".parse().unwrap());
    let report = scheduler.tick().await.unwrap();
    assert_eq!(report.delivered, 1);
    assert_eq!(notifier.messages_to(OWNER), vec!["оформить invoice"]);

    // 4. And it is queued for the same day next month, once.
    assert!(
        scheduler.tick().await.unwrap().is_quiet(),
        "not delivered twice"
    );
    let after = h.repository.get(&stored[0].id).await.unwrap().unwrap();
    assert_eq!(
        after.next_fire_at,
        Some("2026-04-20T06:00:00Z".parse().unwrap())
    );
}

#[tokio::test]
async fn the_model_is_asked_in_the_wire_format_it_expects() {
    let server = model_extracting(invoice_schedule()).await;
    let h = harness(&server).await;
    respond(
        &h.service,
        OWNER,
        &Command::Reminder("каждый месяц 20 число".into()),
    )
    .await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();

    // The system prompt leads, carries the local date so "tomorrow" is resolvable, and the
    // extraction function is offered.
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(
        body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("2026-03-10")
    );
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["tools"][0]["function"]["name"], "schedule_reminder");
    assert_eq!(body["temperature"], 0.0);
}

#[tokio::test]
async fn a_reminder_listed_and_then_deleted_leaves_the_database_empty() {
    let server = model_extracting(invoice_schedule()).await;
    let h = harness(&server).await;

    let created = respond(
        &h.service,
        OWNER,
        &Command::Reminder("каждый месяц 20".into()),
    )
    .await;
    let id = id_from(&created);

    let listing = respond(&h.service, OWNER, &Command::Reminders).await;
    assert!(listing.contains("оформить invoice"), "{listing}");
    assert!(listing.contains(&id), "{listing}");

    let deleted = respond(&h.service, OWNER, &Command::ReminderDelete(id)).await;
    assert!(deleted.contains("Удалил"), "{deleted}");
    assert!(h.repository.list_for_owner(OWNER).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_one_off_is_delivered_once_and_then_stops_for_good() {
    let server = model_extracting(serde_json::json!({
        "kind": "once", "time": "09:00", "date": "2026-03-15", "text": "позвонить в банк"
    }))
    .await;
    let h = harness(&server).await;
    respond(
        &h.service,
        OWNER,
        &Command::Reminder("15 марта в 9 позвонить в банк".into()),
    )
    .await;

    let notifier = Arc::new(RecordingNotifier::new());
    let scheduler = SchedulerService::new(
        h.repository.clone(),
        Arc::clone(&notifier),
        Arc::clone(&h.clock),
    );

    h.clock.set("2026-03-15T05:00:00Z".parse().unwrap());
    assert_eq!(scheduler.tick().await.unwrap().delivered, 1);
    assert_eq!(notifier.messages_to(OWNER), vec!["позвонить в банк"]);

    // A year later it is still silent.
    h.clock.set("2027-03-15T05:00:00Z".parse().unwrap());
    assert!(scheduler.tick().await.unwrap().is_quiet());
    assert_eq!(notifier.messages_to(OWNER).len(), 1);
}

#[tokio::test]
async fn a_month_of_downtime_produces_one_message_not_thirty() {
    let server = model_extracting(serde_json::json!({
        "kind": "daily", "time": "10:00", "text": "зарядка"
    }))
    .await;
    let h = harness(&server).await;
    respond(
        &h.service,
        OWNER,
        &Command::Reminder("каждый день в 10".into()),
    )
    .await;

    // The process was gone for a month.
    let notifier = Arc::new(RecordingNotifier::new());
    let scheduler = SchedulerService::new(
        h.repository.clone(),
        Arc::clone(&notifier),
        Arc::clone(&h.clock),
    );
    h.clock.set("2026-04-10T08:00:00Z".parse().unwrap());

    assert_eq!(scheduler.tick().await.unwrap().delivered, 1);
    assert_eq!(
        notifier.messages_to(OWNER).len(),
        1,
        "one greeting, not a month of them"
    );
}

#[tokio::test]
async fn a_model_outage_leaves_nothing_behind_and_says_so() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let h = harness(&server).await;

    let reply = respond(
        &h.service,
        OWNER,
        &Command::Reminder("каждый день в 10".into()),
    )
    .await;
    assert!(reply.contains("Модель сейчас недоступна"), "{reply}");
    assert!(h.repository.list_for_owner(OWNER).await.unwrap().is_empty());
}

#[tokio::test]
async fn reminders_from_before_a_restart_still_fire_after_it() {
    let server = model_extracting(invoice_schedule()).await;
    let h = harness(&server).await;
    respond(
        &h.service,
        OWNER,
        &Command::Reminder("каждый месяц 20 число".into()),
    )
    .await;

    // Everything the first process held is dropped; only the file remains.
    drop(h.service);
    drop(h.repository);

    let restarted = SqliteReminderRepository::open(&h.database).await.unwrap();
    let notifier = Arc::new(RecordingNotifier::new());
    let scheduler = SchedulerService::new(restarted, Arc::clone(&notifier), Arc::clone(&h.clock));

    h.clock.set("2026-03-20T06:00:00Z".parse().unwrap());
    assert_eq!(scheduler.tick().await.unwrap().delivered, 1);
    assert_eq!(notifier.messages_to(OWNER), vec!["оформить invoice"]);
}
