//! What one conversation is about.
//!
//! [`super::Chat`] owns everything that is the same whatever the person is talking about:
//! loading the history, putting the request to the model, keeping the alternation legal,
//! compacting when it grows. A `Capability` supplies the three things that differ — the
//! instructions, the tools, and what running one of them means.
//!
//! The split exists because those invariants are easy to break and expensive to break
//! twice. A second conversation loop would be a second place for a tool call to be left
//! without a result.

use async_trait::async_trait;

use serana_domain::reminder::UserId;
use serana_domain::tool::ToolSpec;
use serana_domain::{LlmError, StorageError};

/// What the loop itself needs to be able to report, whatever the capability's own failures
/// are. Implemented by each capability's error type, so the loop can fail without knowing
/// what it is driving.
pub trait TurnError: From<LlmError> + From<StorageError> {
    /// The model's answer could not be used — an argument object that does not fit, a
    /// request with nothing in it. The message is shown to the person, because it is what
    /// tells them what to rephrase.
    fn malformed(reason: String) -> Self;
}

/// One subject the assistant can be asked about.
#[async_trait]
pub trait Capability: Send + Sync {
    /// What a turn produced, in whatever shape the frontend needs to render it.
    type Outcome: Send;
    type Error: TurnError + Send;

    /// A short, stable word naming the subject. It scopes the stored conversation, so two
    /// capabilities never share a history — and, more to the point, never inherit each
    /// other's system prompt, which is stored with it and never mutated afterwards.
    fn topic(&self) -> &'static str;

    /// The system prompt. Byte-stable for the life of a conversation: anything that changes
    /// between turns belongs in [`Self::context`] instead.
    fn instructions(&self) -> &'static str;

    /// Every tool offered on this turn.
    fn tools(&self) -> Vec<ToolSpec>;

    /// Whether `name` is one of ours, so a hallucinated tool name is never dispatched.
    fn knows(&self, name: &str) -> bool;

    /// The moment this turn happens. Read once and passed everywhere, so nothing in a turn
    /// disagrees with anything else about what time it is.
    fn now(&self) -> jiff::Timestamp;

    /// The per-turn facts, prefixed to what the person wrote.
    ///
    /// This is the sanctioned injection point: the system prompt stays untouched, and
    /// anything that has to reach the model mid-conversation rides a user message.
    async fn context(&self, owner: UserId, now: jiff::Timestamp) -> Result<String, Self::Error>;

    /// Carry out the tool the model chose. Only ever called with a name [`Self::knows`].
    async fn run(
        &self,
        owner: UserId,
        now: jiff::Timestamp,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<Self::Outcome, Self::Error>;

    /// The outcome for a turn where the model answered in prose rather than acting — a
    /// question back, or an explanation of what it could not work out.
    fn said(&self, words: String) -> Self::Outcome;

    /// What the model is told came of its call, as the tool result for the next turn.
    ///
    /// Short on purpose: it is re-sent with every subsequent turn, so anything verbose here
    /// is paid for over and over.
    fn summarise(&self, outcome: &Self::Outcome) -> String;
}
