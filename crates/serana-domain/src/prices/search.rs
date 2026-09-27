//! Turning what someone called an asset into the one they meant.
//!
//! "Add 1inch" arrives as words, and a price source's search answers with every asset whose
//! name so much as contains them — the real token, its vault, its wrapped copy, and a long
//! tail of lookalikes sharing its ticker. Picking one is a rule, not a guess, so it lives
//! here where it can be tested without a network.

use serde::{Deserialize, Serialize};

use super::quote::AssetId;

/// One asset a search turned up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetMatch {
    pub id: AssetId,
    pub name: String,
    pub symbol: String,
    /// Position by market capitalisation, 1 being the largest. `None` for the unranked
    /// tail, which is where lookalikes live.
    pub rank: Option<u32>,
}

/// Words people put around a name that no search index knows. "1inch token" finds nothing;
/// "1inch" finds it.
const FILLER: &[&str] = &["token", "tokens", "coin", "coins", "price", "prices", "the"];

/// The query with the filler taken out, or `None` when nothing is left to search for.
pub fn search_terms(raw: &str) -> Option<String> {
    let terms: Vec<&str> = raw
        .split_whitespace()
        .filter(|word| !FILLER.contains(&word.to_lowercase().as_str()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

/// Which of `matches` was meant by `query`.
///
/// In order of preference: a ticker that is exactly the query, then a name or id that is,
/// then whatever the source ranked first. Within each, the larger market cap wins — a ticker
/// is not unique, and the asset someone means by "COW" is the one with a market, not the
/// memecoin that borrowed its symbol last week.
pub fn best_match<'a>(query: &str, matches: &'a [AssetMatch]) -> Option<&'a AssetMatch> {
    let wanted = query.trim().to_lowercase();
    let exact = |pick: fn(&AssetMatch) -> String| {
        matches
            .iter()
            .filter(|candidate| pick(candidate) == wanted)
            .min_by_key(|candidate| candidate.rank.unwrap_or(u32::MAX))
    };

    exact(|m| m.symbol.trim().to_lowercase())
        .or_else(|| exact(|m| m.name.trim().to_lowercase()))
        .or_else(|| exact(|m| m.id.as_str().to_lowercase()))
        .or_else(|| matches.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(id: &str, name: &str, symbol: &str, rank: Option<u32>) -> AssetMatch {
        AssetMatch {
            id: AssetId::new(id),
            name: name.into(),
            symbol: symbol.into(),
            rank,
        }
    }

    /// What CoinGecko actually returned for "cow", in its order.
    fn cow() -> Vec<AssetMatch> {
        vec![
            found("memory-cow-moo", "Memory cow Moo", "MOO", Some(1185)),
            found("onecow", "OneCOW", "\u{a0}1COW", Some(3275)),
            found("cow-protocol", "CoW Protocol", "COW", Some(312)),
            found("cow-lookalike", "Cow Token", "COW", None),
        ]
    }

    #[test]
    fn an_exact_ticker_wins_over_whatever_the_source_listed_first() {
        let found = cow();
        let chosen = best_match("cow", &found).unwrap();
        assert_eq!(chosen.id, AssetId::new("cow-protocol"));
    }

    #[test]
    fn a_shared_ticker_goes_to_the_asset_with_a_market() {
        // The lookalike borrowed "COW" and has no rank; the real one has.
        let found = cow();
        let chosen = best_match("COW", &found).unwrap();
        assert_eq!(chosen.rank, Some(312));
    }

    #[test]
    fn a_name_is_enough_when_no_ticker_matches() {
        let found = cow();
        let chosen = best_match("CoW Protocol", &found).unwrap();
        assert_eq!(chosen.id, AssetId::new("cow-protocol"));
    }

    #[test]
    fn with_nothing_exact_the_source_s_own_first_choice_stands() {
        // It orders by relevance; second-guessing that with no better signal is noise.
        let found = cow();
        let chosen = best_match("moo", &found).unwrap();
        assert_eq!(chosen.id, AssetId::new("memory-cow-moo"));
    }

    #[test]
    fn nothing_found_is_nothing_chosen() {
        assert_eq!(best_match("1inch", &[]), None);
    }

    #[test]
    fn a_ticker_padded_with_odd_whitespace_still_matches() {
        // CoinGecko really does send "\u{a0}1COW".
        let found = cow();
        let chosen = best_match("1cow", &found).unwrap();
        assert_eq!(chosen.id, AssetId::new("onecow"));
    }

    #[test]
    fn the_words_people_put_around_a_name_are_dropped_before_searching() {
        // "1inch token" returns nothing from CoinGecko; "1inch" returns the token.
        assert_eq!(search_terms("1inch token").as_deref(), Some("1inch"));
        assert_eq!(search_terms("1inch token price").as_deref(), Some("1inch"));
        assert_eq!(search_terms("the Uniswap coin").as_deref(), Some("Uniswap"));
    }

    #[test]
    fn a_query_that_is_all_filler_is_no_query() {
        assert_eq!(search_terms("token price"), None);
        assert_eq!(search_terms("   "), None);
    }
}
