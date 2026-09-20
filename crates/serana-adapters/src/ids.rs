//! Short, human-retypable identifiers.

use rand::seq::IndexedRandom;
use serana_domain::IdGenerator;

/// Crockford's base32 alphabet: no `I`, `L`, `O` or `U`, so nothing reads as a digit and
/// nothing spells anything.
const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Eight characters of base32 is about 40 bits. For a personal assistant's reminders that
/// is far beyond the point where a collision is plausible, and it still fits in a message.
const LENGTH: usize = 8;

/// Random identifiers.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomIds;

impl IdGenerator for RandomIds {
    fn generate(&self) -> String {
        let mut rng = rand::rng();
        (0..LENGTH)
            .map(|_| {
                *ALPHABET
                    .choose(&mut rng)
                    .expect("the alphabet is never empty") as char
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn an_id_is_the_expected_length_and_alphabet() {
        let id = RandomIds.generate();
        assert_eq!(id.len(), LENGTH);
        assert!(
            id.bytes().all(|b| ALPHABET.contains(&b)),
            "{id} contains characters outside the alphabet"
        );
    }

    #[test]
    fn the_alphabet_excludes_the_characters_people_mistype() {
        for confusable in ["i", "l", "o", "u"] {
            assert!(
                !String::from_utf8_lossy(ALPHABET).contains(confusable),
                "{confusable} is too easy to confuse with a digit"
            );
        }
    }

    #[test]
    fn ids_do_not_repeat_across_a_large_batch() {
        let ids: HashSet<String> = (0..10_000).map(|_| RandomIds.generate()).collect();
        assert_eq!(
            ids.len(),
            10_000,
            "collisions at this scale mean the entropy is wrong"
        );
    }
}
