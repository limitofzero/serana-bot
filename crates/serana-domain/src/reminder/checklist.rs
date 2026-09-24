//! A reminder's checklist, and what counts as ticked.
//!
//! Ticks are timestamps rather than flags, so a recurring checklist comes back empty each
//! period without anything having to reset it. That makes "is this done?" a question about
//! *when* it was ticked, which is why the period arithmetic lives here too.

use serde::{Deserialize, Serialize};

use crate::error::StorageError;

use super::entry::Reminder;

/// One line of a reminder's checklist.
///
/// `done_at` is a moment rather than a flag, because a recurring checklist has to come back
/// unticked next period. Whether it counts as done is therefore a question about *when* it
/// was ticked, answered by [`Reminder::is_done`] — nothing ever has to reset it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    #[serde(default)]
    pub done_at: Option<jiff::Timestamp>,
}

impl TodoItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            done_at: None,
        }
    }
}

impl Reminder {
    /// The start of the period `now` falls in, or `None` if the recurrence has no periods.
    fn period_start(&self, now: jiff::Timestamp) -> Result<Option<jiff::Timestamp>, StorageError> {
        let tz = self.timezone.resolve()?;
        Ok(self.recurrence.period_start_of(now, &tz))
    }

    /// Whether `item` counts as ticked for the period `now` falls in.
    ///
    /// A tick from a previous period does not count — that is the whole reason `done_at` is
    /// a timestamp. A one-off has a single period stretching forever, so any tick counts.
    pub fn is_done(&self, item: &TodoItem, now: jiff::Timestamp) -> Result<bool, StorageError> {
        let Some(done_at) = item.done_at else {
            return Ok(false);
        };
        Ok(match self.period_start(now)? {
            Some(start) => done_at >= start,
            None => true,
        })
    }

    /// The items still outstanding this period, in order.
    pub fn outstanding(&self, now: jiff::Timestamp) -> Result<Vec<&TodoItem>, StorageError> {
        let mut left = Vec::new();
        for item in &self.items {
            if !self.is_done(item, now)? {
                left.push(item);
            }
        }
        Ok(left)
    }

    /// Whether every item has been ticked this period. False when there is no checklist —
    /// an empty list is not "finished", it is a reminder that never had one.
    pub fn all_done(&self, now: jiff::Timestamp) -> Result<bool, StorageError> {
        if self.items.is_empty() {
            return Ok(false);
        }
        Ok(self.outstanding(now)?.is_empty())
    }

    /// Tick the items matching `wanted`, returning the text of each one actually ticked.
    ///
    /// Matching is forgiving because the phrases come from a model repeating the user back:
    /// an exact match first, then a single unambiguous containment. An ambiguous phrase
    /// ticks nothing, because ticking the wrong line is worse than ticking none.
    pub fn complete(&mut self, wanted: &[String], now: jiff::Timestamp) -> Vec<String> {
        let mut ticked = Vec::new();
        for phrase in wanted {
            let needle = phrase.trim().to_lowercase();
            if needle.is_empty() {
                continue;
            }
            let exact: Vec<usize> = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| item.text.trim().to_lowercase() == needle)
                .map(|(index, _)| index)
                .collect();
            let candidates = if exact.is_empty() {
                self.items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        let text = item.text.to_lowercase();
                        text.contains(&needle) || needle.contains(&text)
                    })
                    .map(|(index, _)| index)
                    .collect()
            } else {
                exact
            };

            if let [only] = candidates[..] {
                self.items[only].done_at = Some(now);
                ticked.push(self.items[only].text.clone());
            }
        }
        ticked
    }

    /// Replace the checklist, keeping the ticks of any line that survives the edit.
    ///
    /// Adding a fourth thing to do should not un-tick the three already done. Lines are
    /// matched by their text, which is the only identity a checklist item has — rename one
    /// and it is a new line, which is the honest reading of a rename.
    pub fn relist(&mut self, items: Vec<TodoItem>) {
        let previous = std::mem::take(&mut self.items);
        self.items = items
            .into_iter()
            .map(|mut item| {
                if item.done_at.is_none() {
                    let needle = item.text.trim().to_lowercase();
                    item.done_at = previous
                        .iter()
                        .find(|old| old.text.trim().to_lowercase() == needle)
                        .and_then(|old| old.done_at);
                }
                item
            })
            .collect();
    }

    /// Untick everything, so the checklist reads as fresh for the period.
    pub fn reopen(&mut self) {
        for item in &mut self.items {
            item.done_at = None;
        }
    }
}
