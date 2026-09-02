CREATE TABLE notification_settings (
  user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  telegram_enabled boolean NOT NULL DEFAULT false,
  telegram_bot_token text,
  telegram_chat_id text NOT NULL DEFAULT '',
  email_enabled boolean NOT NULL DEFAULT false,
  smtp_host text NOT NULL DEFAULT '',
  smtp_port integer NOT NULL DEFAULT 587 CHECK (smtp_port BETWEEN 1 AND 65535),
  smtp_tls boolean NOT NULL DEFAULT true,
  smtp_from text NOT NULL DEFAULT '',
  smtp_username text NOT NULL DEFAULT '',
  smtp_password text,
  created_at timestamptz NOT NULL DEFAULT now(),
  updated_at timestamptz NOT NULL DEFAULT now()
);
