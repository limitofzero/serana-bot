//! A price source and a digest store, in memory.

use std::sync::Mutex;

use async_trait::async_trait;
use serana_domain::NotifyError;
use serana_domain::StorageError;
use serana_domain::prices::{
    AssetId, Digest, DigestNotifier, DigestRepository, PriceError, PriceSource, Quote, Snapshot,
};
use serana_domain::reminder::UserId;

/// A [`PriceSource`] answering from a fixed table, and counting how often it was asked.
///
/// The count is the point: the whole case for a cached digest is that asking twice does not
/// read twice.
#[derive(Debug, Default)]
pub struct StubPrices {
    prices: Mutex<Vec<(AssetId, f64)>>,
    calls: Mutex<usize>,
    broken: Option<PriceError>,
}

impl StubPrices {
    pub fn new() -> Self {
        Self::default()
    }

    /// A source that fails whatever it is asked.
    pub fn broken(error: PriceError) -> Self {
        Self {
            broken: Some(error),
            ..Self::default()
        }
    }

    pub fn priced(self, asset: &str, usd: f64) -> Self {
        self.prices
            .lock()
            .expect("not poisoned")
            .push((AssetId::new(asset), usd));
        self
    }

    /// How many times prices were read.
    pub fn calls(&self) -> usize {
        *self.calls.lock().expect("not poisoned")
    }
}

#[async_trait]
impl PriceSource for StubPrices {
    fn name(&self) -> &str {
        "stub"
    }

    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        *self.calls.lock().expect("not poisoned") += 1;
        if let Some(error) = &self.broken {
            return Err(error.clone());
        }
        let prices = self.prices.lock().expect("not poisoned");
        Ok(assets
            .iter()
            .filter_map(|asset| {
                let (_, usd) = prices.iter().find(|(known, _)| known == asset)?;
                Some(Quote {
                    asset: asset.clone(),
                    symbol: asset.as_str().to_uppercase(),
                    usd: *usd,
                    change_24h: Some(1.5),
                })
            })
            .collect())
    }
}

/// Digests in a map.
#[derive(Debug, Default)]
pub struct InMemoryDigestRepository {
    stored: Mutex<Vec<Digest>>,
}

impl InMemoryDigestRepository {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.stored.lock().expect("not poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl DigestRepository for InMemoryDigestRepository {
    async fn get(&self, owner: UserId) -> Result<Option<Digest>, StorageError> {
        Ok(self
            .stored
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|digest| digest.owner == owner)
            .cloned())
    }

    async fn put(&self, digest: &Digest) -> Result<(), StorageError> {
        let mut stored = self.stored.lock().expect("not poisoned");
        stored.retain(|existing| existing.owner != digest.owner);
        stored.push(digest.clone());
        Ok(())
    }

    async fn due_at(&self, now: jiff::Timestamp) -> Result<Vec<Digest>, StorageError> {
        let mut due: Vec<Digest> = self
            .stored
            .lock()
            .expect("not poisoned")
            .iter()
            .filter(|digest| digest.next_fire_at.is_some_and(|at| at <= now))
            .cloned()
            .collect();
        due.sort_by_key(|digest| digest.next_fire_at);
        Ok(due)
    }
}

/// A [`DigestNotifier`] that records what it was asked to deliver.
#[derive(Debug, Default)]
pub struct RecordingDigestNotifier {
    sent: Mutex<Vec<(UserId, Snapshot)>>,
    unreachable: bool,
}

impl RecordingDigestNotifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// A recipient who has blocked the bot: the scheduler switches the digest off rather
    /// than paying for a delivery that can never land.
    pub fn unreachable() -> Self {
        Self {
            unreachable: true,
            ..Self::default()
        }
    }

    pub fn sent(&self) -> Vec<(UserId, Snapshot)> {
        self.sent.lock().expect("not poisoned").clone()
    }
}

#[async_trait]
impl DigestNotifier for RecordingDigestNotifier {
    async fn notify(&self, owner: UserId, snapshot: &Snapshot) -> Result<(), NotifyError> {
        if self.unreachable {
            return Err(NotifyError::Unreachable("blocked".into()));
        }
        self.sent
            .lock()
            .expect("not poisoned")
            .push((owner, snapshot.clone()));
        Ok(())
    }
}
