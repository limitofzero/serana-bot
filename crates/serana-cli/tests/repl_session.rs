//! End-to-end: the real `serana` binary, driven through its stdin.
//!
//! This is the only test that covers `main.rs` — reading the environment, building the
//! graph, and the REPL loop itself. Everything the binary touches is real except the model,
//! which is a mock HTTP server speaking chat-completions.

use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A model that reads every request as "the 20th of each month at 10:00".
async fn model() -> MockServer {
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
                            "name": "create_reminder",
                            "arguments": serde_json::json!({
                                "kind": "monthly",
                                "time": "10:00",
                                "days_of_month": [20],
                                "text": "оформить invoice"
                            }).to_string()
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })))
        .mount(&server)
        .await;
    server
}

/// A model that calls `tool` with `arguments`.
async fn model_calling(tool: &str, arguments: serde_json::Value) -> MockServer {
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
                        "function": { "name": tool, "arguments": arguments.to_string() }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })))
        .mount(&server)
        .await;
    server
}

/// Run the binary against `data_dir`, feed it `input`, and return everything it printed.
async fn session(server: &MockServer, data_dir: &std::path::Path, input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_serana"))
        // The binary calls `dotenvy::dotenv()`, which walks up from the working directory
        // until it finds a `.env`. Run from the temporary directory so it finds none: the
        // developer's own `.env` must not decide whether this test passes.
        .current_dir(data_dir)
        // `env_clear` would drop PATH and the dynamic loader's variables; removing the ones
        // a developer's own shell might have set is enough.
        .env_remove("SERANA_MODEL")
        .env_remove("SERANA_TIMEZONE")
        .env("SERANA_API_KEY", "test-key")
        .env("SERANA_BASE_URL", server.uri())
        .env("SERANA_USER_ID", "42")
        .env("SERANA_DATA_DIR", data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should start");

    child
        .stdin
        .as_mut()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .await
        .expect("the binary should accept input");

    let output = child
        .wait_with_output()
        .await
        .expect("the binary should exit");
    assert!(output.status.success(), "exited with {:?}", output.status);
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
async fn a_reminder_created_in_the_repl_is_listed_back_and_then_deleted() {
    let server = model().await;
    let dir = tempfile::tempdir().unwrap();

    let transcript = session(
        &server,
        dir.path(),
        "/reminder каждый месяц 20 число - оформить invoice\n/reminders\n/quit\n",
    )
    .await;

    // The confirmation reads the schedule back in words.
    assert!(
        transcript.contains("every month on the 20th at 10:00"),
        "{transcript}"
    );
    assert!(transcript.contains("оформить invoice"), "{transcript}");

    let id = transcript
        .lines()
        .find_map(|line| line.trim().strip_prefix("id: "))
        .expect("the confirmation carries an id")
        .trim()
        .to_owned();

    // A second process, against the same data directory, still sees it — so it really went
    // to disk and the environment really pointed at that directory.
    let second = session(&server, dir.path(), "/reminders\n").await;
    assert!(second.contains(&id), "{second}");

    // Removing it is not a command any more: a model that calls delete_reminder with the
    // id it read out of the prompt.
    let deleter = model_calling("delete_reminder", serde_json::json!({ "id": id })).await;
    let third = session(&deleter, dir.path(), "/reminder remove the invoice one\n").await;
    assert!(third.contains("Deleted"), "{third}");

    let fourth = session(&server, dir.path(), "/reminders\n").await;
    assert!(fourth.contains("No reminders yet"), "{fourth}");
}

#[tokio::test]
async fn the_repl_survives_input_it_does_not_understand() {
    let server = model().await;
    let dir = tempfile::tempdir().unwrap();

    let transcript = session(&server, dir.path(), "\n   \n/frobnicate\n/help\n").await;
    assert!(
        transcript.contains("Unknown command \"frobnicate\""),
        "{transcript}"
    );
    // And it kept going rather than exiting on the bad line.
    assert!(
        transcript.contains("/reminders"),
        "help still printed: {transcript}"
    );
}

#[tokio::test]
async fn end_of_input_ends_the_session_without_an_explicit_quit() {
    let server = model().await;
    let dir = tempfile::tempdir().unwrap();
    let transcript = session(&server, dir.path(), "/reminders\n").await;
    assert!(transcript.contains("No reminders yet"), "{transcript}");
}

#[tokio::test]
async fn the_binary_refuses_to_start_without_a_user_id() {
    let server = model().await;
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_serana"))
        // Away from the repository, so `dotenvy` cannot supply the very variable whose
        // absence is under test.
        .current_dir(dir.path())
        .env("SERANA_API_KEY", "test-key")
        .env("SERANA_BASE_URL", server.uri())
        .env("SERANA_DATA_DIR", dir.path())
        .env_remove("SERANA_USER_ID")
        .stdin(Stdio::null())
        .output()
        .await
        .expect("the binary should run");

    assert!(!output.status.success(), "it should exit non-zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("SERANA_USER_ID"), "{stderr}");
}
