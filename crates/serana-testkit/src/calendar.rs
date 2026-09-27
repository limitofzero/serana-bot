//! A calendar in memory.

use std::sync::Mutex;

use async_trait::async_trait;
use serana_domain::calendar::{CalendarError, CalendarEvent, CalendarPort, EventId, NewEvent};

/// A [`CalendarPort`] over a list of events, recording what was done to it.
///
/// Created events are stored as the *blocked* span — the appointment plus its travel — so a
/// test can see what the calendar would actually have reserved, which is the whole point of
/// the padding.
#[derive(Debug, Default)]
pub struct InMemoryCalendar {
    events: Mutex<Vec<CalendarEvent>>,
    calls: Mutex<Vec<(&'static str, EventId)>>,
    /// When set, every call fails with it. For the path where the provider is down.
    broken: Option<CalendarError>,
    next_id: Mutex<u32>,
}

impl InMemoryCalendar {
    pub fn new() -> Self {
        Self::default()
    }

    /// A calendar that fails whatever it is asked.
    pub fn broken(error: CalendarError) -> Self {
        Self {
            broken: Some(error),
            ..Self::default()
        }
    }

    /// Put an event on it before the test starts.
    pub fn with(self, event: CalendarEvent) -> Self {
        self.events.lock().expect("not poisoned").push(event);
        self
    }

    /// Everything still on the calendar.
    pub fn events(&self) -> Vec<CalendarEvent> {
        self.events.lock().expect("not poisoned").clone()
    }

    /// Which ids were struck off, declined or deleted, in order.
    pub fn calls(&self, verb: &str) -> Vec<EventId> {
        self.calls
            .lock()
            .expect("not poisoned")
            .iter()
            .filter(|(seen, _)| *seen == verb)
            .map(|(_, id)| id.clone())
            .collect()
    }

    fn check(&self) -> Result<(), CalendarError> {
        match &self.broken {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn record(&self, verb: &'static str, id: &EventId) {
        self.calls
            .lock()
            .expect("not poisoned")
            .push((verb, id.clone()));
    }
}

#[async_trait]
impl CalendarPort for InMemoryCalendar {
    async fn events_between(
        &self,
        from: jiff::Timestamp,
        to: jiff::Timestamp,
    ) -> Result<Vec<CalendarEvent>, CalendarError> {
        self.check()?;
        let mut found: Vec<CalendarEvent> = self
            .events()
            .into_iter()
            .filter(|event| event.overlaps(from, to))
            .collect();
        found.sort_by_key(|event| event.starts_at);
        Ok(found)
    }

    async fn get(&self, id: &EventId) -> Result<Option<CalendarEvent>, CalendarError> {
        self.check()?;
        Ok(self.events().into_iter().find(|event| &event.id == id))
    }

    async fn create(&self, event: &NewEvent) -> Result<CalendarEvent, CalendarError> {
        self.check()?;
        let mut next = self.next_id.lock().expect("not poisoned");
        *next += 1;
        let (starts_at, ends_at) = event.blocked();
        let created = CalendarEvent {
            id: EventId::new(format!("ev{next}")),
            summary: event.summary.clone(),
            starts_at,
            ends_at,
            all_day: false,
            location: event.location.clone(),
            mine: true,
            recurring: false,
        };
        self.events
            .lock()
            .expect("not poisoned")
            .push(created.clone());
        Ok(created)
    }

    async fn strike_off(&self, id: &EventId) -> Result<(), CalendarError> {
        self.check()?;
        self.record("strike_off", id);
        self.events
            .lock()
            .expect("not poisoned")
            .retain(|event| &event.id != id);
        Ok(())
    }

    async fn decline(&self, id: &EventId) -> Result<(), CalendarError> {
        self.check()?;
        self.record("decline", id);
        Ok(())
    }

    async fn delete(&self, id: &EventId) -> Result<(), CalendarError> {
        self.check()?;
        self.record("delete", id);
        self.events
            .lock()
            .expect("not poisoned")
            .retain(|event| &event.id != id);
        Ok(())
    }
}
