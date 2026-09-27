//! Reading prices, storing the digest, and pushing it out.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{NotifyError, StorageError};
use crate::reminder::UserId;

use super::digest::Digest;
use super::quote::{AssetId, Quote, Snapshot};

/// Why prices could not be read.
///
/// One variant: every caller does the same thing with it — says the market could not be
/// reached and shows what it already had. Splitting it into rate-limited, offline and
/// malformed would produce three arms that all render the same sentence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("could not read prices: {0}")]
pub struct PriceError(pub String);

impl PriceError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// Somewhere prices come from.
///
/// Implementations return **one quote per asset they could price**, in the order asked for,
/// and simply omit the rest rather than inventing a zero. A source that knows two of three
/// assets is useful; a source that reports the third as free is not.
#[async_trait]
pub trait PriceSource: Send + Sync {
    /// What to call this in a digest that had to fall back to it.
    fn name(&self) -> &str;

    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError>;
}

/// Persistence for digests.
#[async_trait]
pub trait DigestRepository: Send + Sync {
    async fn get(&self, owner: UserId) -> Result<Option<Digest>, StorageError>;

    /// Insert or replace.
    async fn put(&self, digest: &Digest) -> Result<(), StorageError>;

    /// Digests whose `next_fire_at` is at or before `now`.
    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Digest>, StorageError>;
}

/// Delivers a digest out of band.
///
/// Takes the snapshot rather than a rendered string for the same reason [`crate::reminder::Notifier`]
/// takes the reminder: composing the sentence is the frontend's job, and the frontend is the
/// only layer that may read `serana-app`.
#[async_trait]
pub trait DigestNotifier: Send + Sync {
    async fn notify(&self, owner: UserId, snapshot: &Snapshot) -> Result<(), NotifyError>;
}

#[async_trait]
impl<T: DigestRepository + ?Sized> DigestRepository for Arc<T> {
    async fn get(&self, owner: UserId) -> Result<Option<Digest>, StorageError> {
        (**self).get(owner).await
    }

    async fn put(&self, digest: &Digest) -> Result<(), StorageError> {
        (**self).put(digest).await
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Digest>, StorageError> {
        (**self).due_at(now).await
    }
}

#[async_trait]
impl<T: PriceSource + ?Sized> PriceSource for Arc<T> {
    fn name(&self) -> &str {
        (**self).name()
    }

    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        (**self).quotes(assets).await
    }
}

#[async_trait]
impl<T: DigestNotifier + ?Sized> DigestNotifier for Arc<T> {
    async fn notify(&self, owner: UserId, snapshot: &Snapshot) -> Result<(), NotifyError> {
        (**self).notify(owner, snapshot).await
    }
}
