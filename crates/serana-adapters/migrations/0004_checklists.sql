-- A reminder can carry a checklist instead of being only a message.
--
-- JSON for the same reason `recurrence` is JSON: nothing queries inside it, and adding a
-- field to an item must not mean a migration. `[]` is the ordinary case — a reminder that
-- is just a message — so every existing row migrates without being touched.
ALTER TABLE reminders ADD COLUMN items TEXT NOT NULL DEFAULT '[]';
