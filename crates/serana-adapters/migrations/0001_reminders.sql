-- Reminders, and the index the scheduler's hot path depends on.

CREATE TABLE IF NOT EXISTS reminders (
    id            TEXT    PRIMARY KEY,
    owner         INTEGER NOT NULL,
    text          TEXT    NOT NULL,
    -- The Recurrence enum as JSON. Kept opaque to SQL: nothing queries inside it, and
    -- adding a variant must not mean a migration.
    recurrence    TEXT    NOT NULL,
    timezone      TEXT    NOT NULL,
    -- Nanoseconds since the Unix epoch. Integers so ordering is numeric; nanoseconds so a
    -- stored instant round-trips to exactly what was written.
    created_at    INTEGER NOT NULL,
    -- NULL means "will never fire again": a spent one-off, or a cancelled reminder.
    next_fire_at  INTEGER,
    last_fired_at INTEGER
) STRICT;

-- The scheduler asks "what is due?" on every tick. Partial, because rows that will never
-- fire again are the ones that accumulate and they are exactly the ones it never wants.
CREATE INDEX IF NOT EXISTS idx_reminders_due
    ON reminders (next_fire_at)
    WHERE next_fire_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_reminders_owner ON reminders (owner);
