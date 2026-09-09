CREATE TABLE wechat_qr_sessions (
  user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  session_id uuid NOT NULL UNIQUE,
  expires_at timestamptz NOT NULL DEFAULT now() + interval '5 minutes',
  created_at timestamptz NOT NULL DEFAULT now()
);
