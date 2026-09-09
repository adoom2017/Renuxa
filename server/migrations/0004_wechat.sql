CREATE TABLE wechat_binding_codes (
  user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  digest text NOT NULL UNIQUE,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE wechat_bindings (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
  gateway_id text NOT NULL, account_id text NOT NULL, sender_id text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE(gateway_id, account_id, sender_id)
);
CREATE TABLE wechat_drafts (
  binding_id uuid PRIMARY KEY REFERENCES wechat_bindings(id) ON DELETE CASCADE,
  fields jsonb NOT NULL DEFAULT '{}',
  version integer NOT NULL DEFAULT 0,
  preview_version integer,
  duplicate_warning boolean NOT NULL DEFAULT false,
  subscription_id uuid REFERENCES subscriptions(id),
  result text,
  expires_at timestamptz NOT NULL DEFAULT now() + interval '24 hours'
);
CREATE TABLE wechat_messages (
  binding_id uuid NOT NULL,
  gateway_id text NOT NULL, account_id text NOT NULL, sender_id text NOT NULL, message_id text NOT NULL,
  result text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY(gateway_id, account_id, sender_id, message_id)
);
CREATE TABLE wechat_rate_limits (
  identity text PRIMARY KEY, window_start timestamptz NOT NULL DEFAULT now(), attempts integer NOT NULL DEFAULT 1
);
CREATE INDEX wechat_messages_created_idx ON wechat_messages(created_at);
