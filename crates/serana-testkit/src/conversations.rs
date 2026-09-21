//! An in-memory [`ConversationRepository`].

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serana_domain::StorageError;
use serana_domain::conversation::{Conversation, ConversationId};
use serana_domain::conversation_store::ConversationRepository;

/// Conversations in a map.
///
/// Ordering of [`ConversationRepository::list`] follows insertion, which is enough for a
/// fake: the real repository orders by recency, and no service depends on the difference.
#[derive(Debug, Default)]
pub struct InMemoryConversationRepository {
    stored: Mutex<HashMap<ConversationId, Conversation>>,
    order: Mutex<Vec<ConversationId>>,
}

impl InMemoryConversationRepository {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.stored.lock().expect("not poisoned").is_empty()
    }

    pub fn len(&self) -> usize {
        self.stored.lock().expect("not poisoned").len()
    }
}

#[async_trait]
impl ConversationRepository for InMemoryConversationRepository {
    async fn load(&self, id: &ConversationId) -> Result<Option<Conversation>, StorageError> {
        Ok(self.stored.lock().expect("not poisoned").get(id).cloned())
    }

    async fn save(&self, conversation: &Conversation) -> Result<(), StorageError> {
        let id = conversation.id().clone();
        let mut order = self.order.lock().expect("not poisoned");
        if !order.contains(&id) {
            order.push(id.clone());
        }
        self.stored
            .lock()
            .expect("not poisoned")
            .insert(id, conversation.clone());
        Ok(())
    }

    async fn delete(&self, id: &ConversationId) -> Result<(), StorageError> {
        self.stored.lock().expect("not poisoned").remove(id);
        self.order.lock().expect("not poisoned").retain(|x| x != id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<ConversationId>, StorageError> {
        Ok(self.order.lock().expect("not poisoned").clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str) -> Conversation {
        Conversation::new(
            ConversationId::new(id),
            "instructions",
            jiff::Timestamp::UNIX_EPOCH,
        )
    }

    #[tokio::test]
    async fn what_is_saved_comes_back() {
        let repo = InMemoryConversationRepository::new();
        let mut c = conversation("tg:1");
        c.push_user("hello").unwrap();
        repo.save(&c).await.unwrap();

        let loaded = repo.load(c.id()).await.unwrap().unwrap();
        assert_eq!(loaded.messages(), c.messages());
        assert_eq!(loaded.system_prompt(), "instructions");
    }

    #[tokio::test]
    async fn saving_twice_replaces_and_does_not_duplicate_the_listing() {
        let repo = InMemoryConversationRepository::new();
        let mut c = conversation("tg:1");
        repo.save(&c).await.unwrap();
        c.push_user("hello").unwrap();
        repo.save(&c).await.unwrap();

        assert_eq!(repo.len(), 1);
        assert_eq!(repo.list().await.unwrap().len(), 1);
        assert_eq!(repo.load(c.id()).await.unwrap().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_unknown_conversation_is_none() {
        let repo = InMemoryConversationRepository::new();
        assert!(
            repo.load(&ConversationId::new("nope"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(repo.is_empty());
    }

    #[tokio::test]
    async fn deleting_removes_it_and_is_idempotent() {
        let repo = InMemoryConversationRepository::new();
        let c = conversation("tg:1");
        repo.save(&c).await.unwrap();
        repo.delete(c.id()).await.unwrap();
        assert!(repo.is_empty());
        assert!(repo.list().await.unwrap().is_empty());
        repo.delete(c.id()).await.unwrap();
    }

    #[tokio::test]
    async fn conversations_are_kept_apart_by_id() {
        let repo = InMemoryConversationRepository::new();
        let mut mine = conversation("tg:1");
        mine.push_user("mine").unwrap();
        let mut theirs = conversation("tg:2");
        theirs.push_user("theirs").unwrap();
        repo.save(&mine).await.unwrap();
        repo.save(&theirs).await.unwrap();

        assert_eq!(repo.len(), 2);
        assert_eq!(
            repo.load(mine.id()).await.unwrap().unwrap().messages(),
            mine.messages()
        );
    }
}
