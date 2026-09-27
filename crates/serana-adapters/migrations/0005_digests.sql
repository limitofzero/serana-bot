-- The price digest: one standing order per person.
--
-- The whole aggregate is one JSON document, like conversations, because nothing in SQL ever
-- looks inside it: the watchlist, the schedule and the cached prices are read and written
-- together or not at all. `next_fire_at` is the single exception, lifted out because the
-- scheduler asks "what is due?" on every tick and that has to be an indexed lookup.
CREATE TABLE IF NOT EXISTS digests (
    owner        INTEGER PRIMARY KEY,
    -- Nanoseconds since the Unix epoch. NULL means the digest is switched off.
    next_fire_at INTEGER,
    document     TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_digests_due
    ON digests (next_fire_at)
    WHERE next_fire_at IS NOT NULL;
