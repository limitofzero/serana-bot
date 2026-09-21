-- Acknowledgement: "I have dealt with this period, stop asking until the next one."
--
-- A watermark rather than a flag. It holds the last instant of the period the user has
-- settled, so the scheduler suppresses what remains of that period and nothing after it.
-- It never needs clearing: once the next period begins the value is in the past and stops
-- having any effect.
ALTER TABLE reminders ADD COLUMN acknowledged_through INTEGER;
