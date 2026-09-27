//! A [`CalendarPort`] over the Google Calendar API.
//!
//! Authenticates with a refresh token obtained once by `scripts/google-oauth.py`. Access
//! tokens live about an hour, so one is fetched on demand and kept until shortly before it
//! expires — refreshing on every call would triple the request count for nothing.

mod wire;

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serana_domain::calendar::{
    CalendarError, CalendarEvent, CalendarPort, Cancellation, EventId, NewEvent,
};

use wire::{WireAttendee, WireEvent, WireEvents, WireToken, describe};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API: &str = "https://www.googleapis.com/calendar/v3";
/// Refresh this long before the token actually expires, so a call that starts just inside
/// the window does not finish just outside it.
const EARLY: Duration = Duration::from_secs(60);

/// How to reach one Google calendar.
#[derive(Debug, Clone)]
pub struct GoogleCalendarConfig {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    /// Which calendar. `primary` is the one belonging to the authorised account.
    pub calendar_id: String,
    pub request_timeout: Duration,
    /// Overridden in tests to point at a mock server.
    pub token_url: String,
    pub api_base: String,
}

impl Default for GoogleCalendarConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            refresh_token: String::new(),
            calendar_id: "primary".to_owned(),
            request_timeout: Duration::from_secs(20),
            token_url: TOKEN_URL.to_owned(),
            api_base: API.to_owned(),
        }
    }
}

impl GoogleCalendarConfig {
    /// Whether enough is configured to try at all.
    pub fn is_configured(&self) -> bool {
        !self.client_id.is_empty()
            && !self.client_secret.is_empty()
            && !self.refresh_token.is_empty()
    }
}

struct Access {
    token: String,
    expires_at: std::time::Instant,
}

/// Google Calendar, behind the domain's port.
pub struct GoogleCalendar {
    http: reqwest::Client,
    config: GoogleCalendarConfig,
    access: Mutex<Option<Access>>,
}

impl GoogleCalendar {
    pub fn new(config: GoogleCalendarConfig) -> Result<Self, CalendarError> {
        if !config.is_configured() {
            return Err(CalendarError::NotConnected);
        }
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        Ok(Self {
            http,
            config,
            access: Mutex::new(None),
        })
    }

    /// A usable access token, minted only when the cached one is gone or nearly so.
    async fn token(&self) -> Result<String, CalendarError> {
        if let Some(access) = self.access.lock().expect("not poisoned").as_ref()
            && access.expires_at > std::time::Instant::now()
        {
            return Ok(access.token.clone());
        }

        let response = self
            .http
            .post(&self.config.token_url)
            .form(&[
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
                ("refresh_token", self.config.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        if !status.is_success() {
            return Err(describe(status.as_u16(), &body));
        }

        let token: WireToken = serde_json::from_str(&body)
            .map_err(|e| CalendarError::Unavailable(format!("unreadable token: {e}")))?;
        let lifetime = Duration::from_secs(token.expires_in.unwrap_or(0));
        *self.access.lock().expect("not poisoned") = Some(Access {
            token: token.access_token.clone(),
            expires_at: std::time::Instant::now() + lifetime.saturating_sub(EARLY),
        });
        Ok(token.access_token)
    }

    fn events_url(&self, suffix: &str) -> String {
        format!(
            "{}/calendars/{}/events{suffix}",
            self.config.api_base.trim_end_matches('/'),
            urlencoding(&self.config.calendar_id)
        )
    }

    /// Send a prepared request, returning the body or a described failure.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<String, CalendarError> {
        let response = request
            .send()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(describe(status.as_u16(), &body))
        }
    }

    /// Fetch one event's raw payload, so a caller can decide what to change about it.
    async fn raw(&self, id: &EventId) -> Result<Option<WireEvent>, CalendarError> {
        let token = self.token().await?;
        let response = self
            .http
            .get(self.events_url(&format!("/{}", urlencoding(id.as_str()))))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        if !status.is_success() {
            return Err(describe(status.as_u16(), &body));
        }
        serde_json::from_str::<WireEvent>(&body)
            .map(Some)
            .map_err(|e| CalendarError::Unavailable(format!("unreadable event: {e}")))
    }

    async fn patch(&self, id: &EventId, body: &serde_json::Value) -> Result<(), CalendarError> {
        let token = self.token().await?;
        self.send(
            self.http
                .patch(self.events_url(&format!("/{}", urlencoding(id.as_str()))))
                .bearer_auth(token)
                .json(body),
        )
        .await
        .map(|_| ())
    }
}

/// Percent-encode a path segment. Calendar ids are email addresses and event ids can carry
/// underscores and digits, so the few characters that matter are encoded by hand rather
/// than taking a dependency for it.
fn urlencoding(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[async_trait]
impl CalendarPort for GoogleCalendar {
    async fn events_between(
        &self,
        from: jiff::Timestamp,
        to: jiff::Timestamp,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        let token = self.token().await?;
        let body = self
            .send(
                self.http
                    .get(self.events_url(""))
                    .bearer_auth(token)
                    .query(&[
                        ("timeMin", from.to_string()),
                        ("timeMax", to.to_string()),
                        // Expand repeats into occurrences: the person asks about days, not
                        // about rules.
                        ("singleEvents", "true".to_owned()),
                        ("orderBy", "startTime".to_owned()),
                        ("maxResults", "100".to_owned()),
                    ]),
            )
            .await?;

        let events: WireEvents = serde_json::from_str(&body)
            .map_err(|e| CalendarError::Unavailable(format!("unreadable events: {e}")))?;
        Ok(events
            .items
            .into_iter()
            // A cancelled occurrence still comes back in the list; it is not on the day.
            .filter(|e| e.status.as_deref() != Some("cancelled"))
            .filter_map(WireEvent::into_domain)
            .collect())
    }

    async fn get(&self, id: &EventId) -> Result<Option<CalendarEvent>, CalendarError> {
        Ok(self.raw(id).await?.and_then(WireEvent::into_domain))
    }

    async fn create(&self, event: &NewEvent) -> Result<CalendarEvent, CalendarError> {
        let token = self.token().await?;
        let body = self
            .send(
                self.http
                    .post(self.events_url(""))
                    .bearer_auth(token)
                    .json(&WireEvent::from(event)),
            )
            .await?;
        serde_json::from_str::<WireEvent>(&body)
            .ok()
            .and_then(WireEvent::into_domain)
            .ok_or_else(|| {
                CalendarError::Unavailable("the calendar did not describe what it created".into())
            })
    }

    async fn strike_off(&self, id: &EventId) -> Result<(), CalendarError> {
        self.patch(id, &serde_json::json!({ "status": "cancelled" }))
            .await
    }

    async fn decline(&self, id: &EventId) -> Result<(), CalendarError> {
        // Only our own attendee row may be touched. Sending the whole list back with one
        // row changed is how Google's API expects this, so the rest is preserved verbatim.
        let Some(event) = self.raw(id).await? else {
            return Err(CalendarError::Unavailable(format!("no event {id}")));
        };
        let attendees: Vec<WireAttendee> = event
            .attendees
            .into_iter()
            .map(|mut a| {
                if a.is_self {
                    a.response_status = Some("declined".to_owned());
                }
                a
            })
            .collect();
        if attendees.is_empty() {
            // Nobody to decline to: this is ours, and striking it off is what "cancel"
            // means for it. The caller should have checked, so say so plainly.
            return Err(CalendarError::Unavailable(
                "this event has no invitation to decline".into(),
            ));
        }
        self.patch(id, &serde_json::json!({ "attendees": attendees }))
            .await
    }

    async fn delete(&self, id: &EventId) -> Result<(), CalendarError> {
        let token = self.token().await?;
        let response = self
            .http
            .delete(self.events_url(&format!("/{}", urlencoding(id.as_str()))))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| CalendarError::Unavailable(e.to_string()))?;
        // Already gone is the outcome asked for, not a failure.
        if response.status() == reqwest::StatusCode::NOT_FOUND || response.status().is_success() {
            return Ok(());
        }
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        Err(describe(status, &body))
    }
}

/// How cancelling this event has to be carried out. Re-exported so a caller can decide
/// between [`CalendarPort::strike_off`] and [`CalendarPort::decline`] without guessing.
pub fn cancellation(event: &CalendarEvent) -> Cancellation {
    event.cancellation()
}

#[cfg(test)]
mod tests;
