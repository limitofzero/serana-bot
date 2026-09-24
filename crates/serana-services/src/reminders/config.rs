//! What a reminder turn is configured with.

use serana_domain::reminder::TimeZoneName;

/// Roughly a hundred and fifty exchanges at this assistant's size.
pub const DEFAULT_COMPACT_ABOVE_TOKENS: u64 = 16_000;

#[derive(Debug, Clone)]
pub struct ReminderConfig {
    /// Model used to read the request. A small one is enough; this is extraction, not
    /// reasoning.
    pub model: String,
    /// Zone every new reminder is created in.
    pub default_timezone: TimeZoneName,
    /// Model for summarising when a conversation is compacted. A smaller one is plenty:
    /// nobody reads the summary but the model itself. Falls back to [`Self::model`].
    pub summary_model: Option<String>,
    /// Compact once the provider reports a prompt above this many tokens.
    ///
    /// Deliberately far off. Providers cache the prompt prefix, so a long *stable* history
    /// costs much less than its token count suggests — while compaction costs a model call
    /// and throws that cache away. Compacting early is the expensive choice.
    pub compact_above_tokens: u64,
    /// Sampling temperature, or `None` to let the provider use its default.
    ///
    /// Extraction wants determinism, so 0 is the natural choice — but reasoning models
    /// reject any value but their default and fail the whole request with a 400. Leaving
    /// this unset is therefore the only setting that works everywhere; models that do
    /// honour it can be given a value.
    pub temperature: Option<f32>,
}

impl ReminderConfig {
    /// The model used for summarising, falling back to the main one.
    pub fn summary_model(&self) -> &str {
        self.summary_model.as_deref().unwrap_or(&self.model)
    }
}
