//! Why a price turn failed.

use serana_domain::prices::PriceError;
use serana_domain::{LlmError, StorageError};

#[derive(Debug, thiserror::Error)]
pub enum PriceTurnError {
    /// The request could not be used. The message is written to be shown to the person,
    /// because it is what tells them what to rephrase.
    #[error("could not understand that: {0}")]
    Unparsable(String),

    /// No source would give a price. Distinct from the rest because there is usually still
    /// something cached to show alongside saying so.
    #[error(transparent)]
    Market(#[from] PriceError),

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl crate::chat::TurnError for PriceTurnError {
    fn malformed(reason: String) -> Self {
        Self::Unparsable(reason)
    }
}
