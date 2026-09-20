//! Semantic retrieval: the ports a vector-backed memory needs.
//!
//! Nothing implements these yet. They exist now because they constrain the shape of
//! [`crate::memory::MemoryRepository`]: a vector implementation of `search` is an
//! [`EmbeddingProvider`] plus a [`Retriever`], and if those did not compose with the memory
//! port, we would find out after writing the filesystem one.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::RetrievalError;

/// A fixed-width vector representation of a piece of text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    pub fn new(values: Vec<f32>) -> Self {
        Self(values)
    }

    pub fn dimensions(&self) -> usize {
        self.0.len()
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// Cosine similarity in `[-1, 1]`.
    ///
    /// `None` when the vectors have different dimensions, or when either is the zero
    /// vector — both are backend bugs the caller must not silently rank on.
    pub fn cosine_similarity(&self, other: &Self) -> Option<f32> {
        if self.0.len() != other.0.len() || self.0.is_empty() {
            return None;
        }
        let dot: f32 = self.0.iter().zip(&other.0).map(|(a, b)| a * b).sum();
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let (a, b) = (norm(&self.0), norm(&other.0));
        if a == 0.0 || b == 0.0 {
            return None;
        }
        Some(dot / (a * b))
    }
}

/// A chunk of source text with a relevance score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Passage {
    /// Where this came from — a memory id, a file path, a URL. Carried so the model can
    /// cite it and the user can check it.
    pub source: String,
    pub content: String,
    pub score: f32,
}

/// Turns text into vectors.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Width of the vectors this provider returns. An index built at one width cannot be
    /// queried at another, so callers check it rather than discovering it through garbage
    /// rankings.
    fn dimensions(&self) -> usize;

    /// Embed a batch. Returns one embedding per input, in order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Embedding>, RetrievalError>;
}

/// Finds passages relevant to a query.
#[async_trait]
pub trait Retriever: Send + Sync {
    /// Ranked best first, at most `limit` results.
    async fn retrieve(&self, query: &str, limit: usize) -> Result<Vec<Passage>, RetrievalError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_are_maximally_similar() {
        let a = Embedding::new(vec![1.0, 2.0, 3.0]);
        let similarity = a.cosine_similarity(&a).unwrap();
        assert!((similarity - 1.0).abs() < 1e-6, "{similarity}");
    }

    #[test]
    fn orthogonal_vectors_score_zero_and_opposite_ones_score_minus_one() {
        let a = Embedding::new(vec![1.0, 0.0]);
        let b = Embedding::new(vec![0.0, 1.0]);
        let c = Embedding::new(vec![-1.0, 0.0]);
        assert!(a.cosine_similarity(&b).unwrap().abs() < 1e-6);
        assert!((a.cosine_similarity(&c).unwrap() + 1.0).abs() < 1e-6);
    }

    #[test]
    fn magnitude_does_not_affect_similarity() {
        let a = Embedding::new(vec![1.0, 1.0]);
        let scaled = Embedding::new(vec![10.0, 10.0]);
        assert!((a.cosine_similarity(&scaled).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mismatched_widths_and_zero_vectors_refuse_to_rank() {
        let a = Embedding::new(vec![1.0, 2.0, 3.0]);
        assert_eq!(a.cosine_similarity(&Embedding::new(vec![1.0, 2.0])), None);
        assert_eq!(
            a.cosine_similarity(&Embedding::new(vec![0.0, 0.0, 0.0])),
            None
        );
        let empty = Embedding::new(vec![]);
        assert_eq!(empty.cosine_similarity(&empty), None);
    }

    #[test]
    fn an_embedding_reports_its_width() {
        assert_eq!(Embedding::new(vec![0.0; 1536]).dimensions(), 1536);
    }
}
