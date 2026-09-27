//! One turn of conversation, whatever it is about.
//!
//! The history is loaded, the request is put to the provider with the capability's toolset,
//! whatever it chose is handed back to the capability, and the exchange is written down. It
//! knows how a request arrives and what to do with the answer; it knows nothing about what
//! is being arranged.

use std::sync::Arc;

use serana_domain::conversation::{Conversation, ConversationId};
use serana_domain::conversation_store::ConversationRepository;
use serana_domain::llm::{CompletionRequest, CompletionResponse, LlmProvider};
use serana_domain::message::Message;
use serana_domain::reminder::UserId;
use serana_domain::{AlternationError, StorageError};

use super::capability::{Capability, TurnError};
use super::config::ChatConfig;

/// What to ask the model for when a conversation has grown too long to keep re-sending.
const SUMMARISE: &str = "Summarise the conversation below so it can stand in for the full \
     history. Keep anything still unresolved — a question you asked that has not been \
     answered, something being discussed — and the decisions already made. Drop \
     pleasantries. Write it as notes, in a few sentences, addressed to yourself.";

/// What the person is told when the model neither acted nor said anything usable.
const NOTHING_USABLE: &str = "I could not work out what you wanted there.";

/// A conversation about one subject.
///
/// The fields are visible inside this crate so a capability's own tests can drive the
/// provider and the store they handed over; nothing outside the crate can reach them.
pub struct Chat<K, L, V> {
    pub(crate) capability: K,
    pub(crate) llm: L,
    pub(crate) conversations: V,
    pub(crate) config: ChatConfig,
}

impl<K, L, V> Chat<K, L, V>
where
    K: Capability,
    L: LlmProvider,
    V: ConversationRepository,
{
    pub fn new(capability: K, llm: L, conversations: V, config: ChatConfig) -> Self {
        Self {
            capability,
            llm,
            conversations,
            config,
        }
    }

    /// What this conversation is about. Listing and the clock come from here; a frontend
    /// needs both and neither is a conversation concern.
    pub fn capability(&self) -> &K {
        &self.capability
    }

    /// How this conversation is configured. The frontend needs the zone to render a time
    /// the same way the model was told it.
    pub fn config(&self) -> &ChatConfig {
        &self.config
    }

    /// Give up the capability, so another conversation can be had over the same storage.
    ///
    /// A turn is one model call, so a test covering two turns needs two chats over one
    /// capability. Consuming rather than cloning keeps that explicit.
    pub fn into_capability(self) -> K {
        self.capability
    }

    /// This capability's stored history for `base`, if there is one.
    ///
    /// The scoping is this type's business, so a test asks with the id it passed to
    /// [`Self::handle`] rather than reconstructing the branch itself.
    #[cfg(test)]
    pub(crate) async fn stored(
        &self,
        base: &ConversationId,
    ) -> Result<Option<Conversation>, StorageError> {
        self.conversations.load(&self.scoped(base)).await
    }

    /// Where this capability's history lives.
    ///
    /// The caller passes one id per person; each capability gets its own branch of it. The
    /// system prompt is stored with the conversation and never changes afterwards, so two
    /// capabilities sharing an id would mean whichever spoke first deciding what the other
    /// one is — along with a toolset that no longer matches the instructions.
    fn scoped(&self, base: &ConversationId) -> ConversationId {
        ConversationId::new(format!("{base}:{}", self.capability.topic()))
    }

    pub async fn handle(
        &self,
        owner: UserId,
        conversation: &ConversationId,
        request: &str,
    ) -> Result<K::Outcome, K::Error> {
        if request.trim().is_empty() {
            return Err(K::Error::malformed("the request is empty".into()));
        }

        let now = self.capability.now();
        let conversation = self.scoped(conversation);
        let mut history = self
            .conversations
            .load(&conversation)
            .await?
            .unwrap_or_else(|| {
                Conversation::new(conversation, self.capability.instructions(), now)
            });

        let context = self.capability.context(owner, now).await?;
        history
            .push_user(format!("{context}\n{request}"))
            .map_err(corrupt::<K::Error>)?;

        let response = self.ask(&history).await?;

        // A name we do not offer is a hallucination, not an action: fall through to the
        // model's own prose rather than dispatching something we cannot check.
        let chosen = response
            .tool_calls
            .iter()
            .find(|call| self.capability.knows(&call.name))
            .cloned();

        let outcome = match &chosen {
            None => self.capability.said(
                response
                    .content
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .unwrap_or(NOTHING_USABLE)
                    .to_owned(),
            ),
            Some(call) => {
                let arguments = call.parse_arguments().map_err(|e| {
                    K::Error::malformed(format!("the answer came back malformed: {e}"))
                })?;
                self.capability
                    .run(owner, now, &call.name, arguments)
                    .await?
            }
        };

        self.record(&mut history, &response, chosen.is_some(), &outcome)?;

        // Compaction is the one sanctioned cache break, so it happens as late as possible:
        // only once the provider says the history it is re-reading has grown past the
        // threshold. `prompt_tokens` is the provider's own count, so there is no tokenizer
        // to take on and no estimate to be wrong about.
        if response.usage.prompt_tokens > self.config.compact_above_tokens {
            self.compact(&mut history).await?;
        }

        self.conversations.save(&history).await?;
        Ok(outcome)
    }

    /// Ask the model, with the whole conversation in front of it.
    async fn ask(&self, history: &Conversation) -> Result<CompletionResponse, K::Error> {
        let mut call = CompletionRequest::new(
            &self.config.model,
            Arc::from(history.system_prompt()),
            history.messages().to_vec(),
        )
        .with_tools(self.capability.tools());
        if let Some(temperature) = self.config.temperature {
            call = call.with_temperature(temperature);
        }
        Ok(self.llm.complete(call).await?)
    }

    /// Append what the model said, and what came of it, to the history.
    ///
    /// A tool call must be followed by exactly one result, on every path — leave one open
    /// and the next turn starts `tool -> user`, which providers reject.
    fn record(
        &self,
        history: &mut Conversation,
        response: &CompletionResponse,
        acted: bool,
        outcome: &K::Outcome,
    ) -> Result<(), K::Error> {
        if !acted {
            return history
                .push_assistant(self.capability.summarise(outcome))
                .map_err(corrupt::<K::Error>);
        }

        let calls = response.tool_calls.clone();
        let Some(first) = calls.first().cloned() else {
            return Ok(());
        };
        history
            .push_tool_calls(calls)
            .map_err(corrupt::<K::Error>)?;
        history
            .push_tool_result(&first.id, self.capability.summarise(outcome))
            .map_err(corrupt::<K::Error>)?;
        // Anything the model asked for beyond the first call was never run; closing them
        // keeps the alternation legal and tells the model why.
        history.close_open_tool_calls("not run: one action per turn");
        Ok(())
    }

    /// Summarise a stored conversation in place, at the person's request.
    ///
    /// Returns whether there was anything to summarise.
    pub async fn compact_conversation(
        &self,
        conversation: &ConversationId,
    ) -> Result<bool, K::Error> {
        let conversation = self.scoped(conversation);
        let Some(mut history) = self.conversations.load(&conversation).await? else {
            return Ok(false);
        };
        if history.is_empty() {
            return Ok(false);
        }
        self.compact(&mut history).await?;
        self.conversations.save(&history).await?;
        Ok(true)
    }

    /// Replace the history with a summary of itself, keeping the system prompt.
    ///
    /// Triggered by [`ChatConfig::compact_above_tokens`], or by the person asking. The
    /// summary arrives as a user message with an assistant acknowledgement after it, so the
    /// conversation is left able to accept the next user message.
    pub async fn compact(&self, history: &mut Conversation) -> Result<(), K::Error> {
        if history.is_empty() {
            return Ok(());
        }

        let summary = self
            .llm
            .complete(CompletionRequest::new(
                self.config.summary_model(),
                Arc::from(SUMMARISE),
                vec![Message::user(transcript(history))],
            ))
            .await?
            .content
            .unwrap_or_default();

        // Same id, same system prompt, same creation time: it is the same conversation,
        // with its middle replaced.
        let mut compacted = Conversation::new(
            history.id().clone(),
            history.system_prompt(),
            history.created_at(),
        );
        compacted
            .push_user(format!(
                "Notes on our earlier conversation:\n{}",
                summary.trim()
            ))
            .map_err(corrupt::<K::Error>)?;
        compacted
            .push_assistant("Noted.")
            .map_err(corrupt::<K::Error>)?;
        *history = compacted;
        Ok(())
    }
}

/// The history as plain lines, for the model to summarise.
fn transcript(history: &Conversation) -> String {
    history
        .messages()
        .iter()
        .map(|message| match message {
            Message::User { content } => format!("them: {content}"),
            Message::Assistant {
                content,
                tool_calls,
            } => match content {
                Some(text) => format!("you: {text}"),
                None => format!(
                    "you: (called {})",
                    tool_calls
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            Message::Tool { content, .. } => format!("result: {content}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An alternation failure is a bug in this loop, not something the person did. It is
/// reported as corrupt storage because that is the one thing a frontend can say about it.
fn corrupt<E: TurnError>(error: AlternationError) -> E {
    E::from(StorageError::Corrupt(error.to_string()))
}
