-- Existing subscriptions have no known start date; preserve their renewal schedule.
ALTER TABLE subscriptions ADD COLUMN start_date date;
ALTER TABLE bills ADD COLUMN source text NOT NULL DEFAULT 'renewal'
  CHECK (source IN ('renewal', 'schedule', 'manual'));
