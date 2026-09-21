-- Conversations, so a follow-up question can be answered by the next message.
--
-- The whole aggregate is one JSON document rather than a row per message. `Conversation`
-- has private fields and only accepts appends through methods that enforce role
-- alternation, so rebuilding one from rows would mean a constructor that bypasses the very
-- invariants the type exists to hold. Serde round-trips it exactly, including the system
-- prompt, which must come back byte for byte or the provider's prompt cache misses on
-- every turn (docs/reference-notes.md §2).
CREATE TABLE IF NOT EXISTS conversations (
    id            TEXT    PRIMARY KEY,
    document      TEXT    NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;

-- `list` orders by recency.
CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations (updated_at DESC);
