//! What each person was last talking about.
//!
//! The assistant keeps one conversation per subject, so a message that is not a command has
//! to be aimed at one of them. Aiming it at whatever they were last talking about is what
//! makes a question answerable: the calendar asks "delete this for good?" and the answer
//! arrives as a plain "yes".
//!
//! Held in memory on purpose. Losing it on a restart costs one message landing on
//! reminders, which is where it would have landed anyway before this existed — and a slash
//! command always says where it is going, so nothing is ever stuck.

use std::collections::HashMap;
use std::sync::Mutex;

use serana_domain::reminder::UserId;

use crate::command::{Command, Topic};

#[derive(Debug, Default)]
pub struct Topics(Mutex<HashMap<UserId, Topic>>);

impl Topics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember what `command` was about, when it was about anything.
    pub fn note(&self, user: UserId, command: &Command) {
        if let Some(topic) = command.topic() {
            self.0.lock().expect("not poisoned").insert(user, topic);
        }
    }

    /// What a bare message from `user` continues.
    pub fn last(&self, user: UserId) -> Topic {
        self.0
            .lock()
            .expect("not poisoned")
            .get(&user)
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: UserId = UserId::new(42);
    const OTHER: UserId = UserId::new(7);

    #[test]
    fn someone_who_has_said_nothing_yet_is_talking_about_reminders() {
        assert_eq!(Topics::new().last(OWNER), Topic::Reminders);
    }

    #[test]
    fn a_bare_message_continues_the_last_subject() {
        let topics = Topics::new();
        topics.note(OWNER, &Command::Calendar("what's on?".into()));
        assert_eq!(topics.last(OWNER), Topic::Calendar);

        topics.note(OWNER, &Command::Reminder("remind me".into()));
        assert_eq!(topics.last(OWNER), Topic::Reminders);
    }

    #[test]
    fn help_and_compact_leave_the_aim_where_it_was() {
        let topics = Topics::new();
        topics.note(OWNER, &Command::Calendar("what's on?".into()));
        topics.note(OWNER, &Command::Help);
        topics.note(OWNER, &Command::Compact);
        assert_eq!(topics.last(OWNER), Topic::Calendar);
    }

    #[test]
    fn one_person_s_subject_is_not_another_s() {
        let topics = Topics::new();
        topics.note(OWNER, &Command::Calendar("what's on?".into()));
        assert_eq!(topics.last(OTHER), Topic::Reminders);
    }
}
