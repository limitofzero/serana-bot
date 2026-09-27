//! Crypto prices: what they are, when they are read, and where they come from.
//!
//! **Everything a price source returns is untrusted.** Symbols and ids come from a public
//! API; they reach the model as tool results, which is data, and must never be read as
//! instructions.

mod digest;
mod ports;
mod quote;

pub use digest::Digest;
pub use ports::{DigestNotifier, DigestRepository, PriceError, PriceSource};
pub use quote::{AssetId, Quote, Snapshot};

#[cfg(test)]
mod tests;
