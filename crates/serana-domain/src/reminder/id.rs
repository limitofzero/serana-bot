//! How a reminder, its owner and its time zone are named.

use serde::{Deserialize, Serialize};

use crate::error::StorageError;

/// Whoever owns a reminder. Currently a Telegram user id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UserId(i64);

impl UserId {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn get(&self) -> i64 {
        self.0
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReminderId(String);

impl ReminderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ReminderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An IANA time zone name, kept as text so it survives a round trip through storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeZoneName(String);

impl TimeZoneName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolve against the tzdb.
    ///
    /// Fallible because a zone name read back from storage may no longer exist — tzdb drops
    /// and renames zones — and a reminder that silently moved to UTC is worse than one that
    /// reports the problem.
    pub fn resolve(&self) -> Result<jiff::tz::TimeZone, StorageError> {
        jiff::tz::TimeZone::get(&self.0)
            .map_err(|e| StorageError::Corrupt(format!("unknown time zone {:?}: {e}", self.0)))
    }
}

impl std::fmt::Display for TimeZoneName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
