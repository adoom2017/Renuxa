ALTER TABLE notification_deliveries
  DROP CONSTRAINT notification_deliveries_channel_check;

ALTER TABLE notification_deliveries
  ADD CONSTRAINT notification_deliveries_channel_check
  CHECK (channel IN ('in_app', 'email', 'telegram'));
