//! Predictable identifiers.

use std::sync::atomic::{AtomicUsize, Ordering};

use serana_domain::IdGenerator;

/// Hands out `r1`, `r2`, `r3`, ... so a test can name the entity it just created.
#[derive(Debug)]
pub struct SequentialIds {
    prefix: String,
    next: AtomicUsize,
}

impl Default for SequentialIds {
    fn default() -> Self {
        Self::new("r")
    }
}

impl SequentialIds {
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
            next: AtomicUsize::new(1),
        }
    }
}

impl IdGenerator for SequentialIds {
    fn generate(&self) -> String {
        format!(
            "{}{}",
            self.prefix,
            self.next.fetch_add(1, Ordering::Relaxed)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_count_up_from_one() {
        let ids = SequentialIds::default();
        assert_eq!(ids.generate(), "r1");
        assert_eq!(ids.generate(), "r2");
        assert_eq!(ids.generate(), "r3");
    }

    #[test]
    fn the_prefix_is_configurable() {
        let ids = SequentialIds::new("task-");
        assert_eq!(ids.generate(), "task-1");
    }
}
