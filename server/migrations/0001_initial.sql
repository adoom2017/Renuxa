CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE users (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(), email text NOT NULL UNIQUE, password_hash text NOT NULL,
  locale text NOT NULL DEFAULT 'zh-CN', timezone text NOT NULL DEFAULT 'Asia/Shanghai', base_currency char(3) NOT NULL DEFAULT 'CNY',
  default_reminder_hour smallint NOT NULL DEFAULT 9, email_verified_at timestamptz, created_at timestamptz NOT NULL DEFAULT now(), deleted_at timestamptz
);
CREATE TABLE subscriptions (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(), user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name text NOT NULL, plan_name text, amount numeric(20,6) NOT NULL CHECK (amount >= 0), currency char(3) NOT NULL,
  cadence_unit text NOT NULL CHECK (cadence_unit IN ('day','week','month','quarter','year','once')), cadence_interval integer NOT NULL DEFAULT 1 CHECK (cadence_interval > 0),
  next_billing_date date NOT NULL, anchor_day smallint NOT NULL CHECK (anchor_day BETWEEN 1 AND 31),
  status text NOT NULL DEFAULT 'active' CHECK (status IN ('active','paused','cancelled','archived')), category text NOT NULL DEFAULT '其他',
  payment_method text, notes text, icon_url text, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX subscriptions_due_idx ON subscriptions(status,next_billing_date);
CREATE INDEX subscriptions_user_idx ON subscriptions(user_id,status);
CREATE TABLE subscription_reminders (
  subscription_id uuid NOT NULL REFERENCES subscriptions(id) ON DELETE CASCADE, days_before integer NOT NULL CHECK (days_before BETWEEN 0 AND 365),
  PRIMARY KEY(subscription_id,days_before)
);
CREATE TABLE exchange_rates (
  rate_date date NOT NULL, base_currency char(3) NOT NULL, quote_currency char(3) NOT NULL, rate numeric(24,12) NOT NULL, provider text NOT NULL,
  PRIMARY KEY(rate_date,base_currency,quote_currency)
);
CREATE TABLE bills (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(), user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE, subscription_id uuid NOT NULL REFERENCES subscriptions(id),
  amount numeric(20,6) NOT NULL, currency char(3) NOT NULL, due_date date NOT NULL, status text NOT NULL DEFAULT 'estimated' CHECK(status IN ('estimated','paid','skipped','refunded')),
  base_amount numeric(20,6), base_currency char(3), exchange_rate numeric(24,12), exchange_rate_date date, idempotency_key text UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX bills_user_due_idx ON bills(user_id,due_date DESC);
CREATE TABLE notifications (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(), user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE, subscription_id uuid REFERENCES subscriptions(id),
  title text NOT NULL, body text NOT NULL, kind text NOT NULL, scheduled_for timestamptz NOT NULL, read_at timestamptz,
  idempotency_key text UNIQUE, created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX notifications_user_idx ON notifications(user_id,scheduled_for DESC);
CREATE TABLE notification_deliveries (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(), notification_id uuid NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
  channel text NOT NULL CHECK(channel IN ('in_app','email')), status text NOT NULL DEFAULT 'pending', attempts integer NOT NULL DEFAULT 0,
  next_attempt_at timestamptz, provider_response text, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now()
);

