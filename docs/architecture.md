# 系统架构

## 服务边界

| 目录 / 服务 | 职责 |
| --- | --- |
| `app/` | Web 与桌面端共用的 React 产品界面 |
| `desktop/` | Vite 桌面 Web 入口 |
| `src-tauri/` | Tauri 原生桌面壳 |
| `server/` / API | Axum HTTP API、认证、订阅与微信业务状态机 |
| `server/` / Worker | 生成账单和通知、发送邮件与 Telegram 通知、清理过期状态 |
| `im-channel-gateway/` | 微信 iLink 和 Telegram 消息接入、媒体转换与消息转发 |
| PostgreSQL | 用户、订阅、账单、通知、通知设置及微信会话状态 |

Web 通过 Nginx 将同源 `/api` 请求转发到 API。Gateway 通过受 token 保护的
HTTP SSE 接口向 API 投递微信消息；API 通过 Gateway 管理接口创建和轮询登录二维码。

## 微信数据流

1. 已认证用户请求 `/api/integrations/wechat/qrcode`。
2. API 向 Gateway 获取二维码，将会话 ID 与当前用户写入 `wechat_qr_sessions`。
3. Web 轮询状态；微信确认后，API 将扫码用户、机器人账号和 Renuxa 用户写入
   `wechat_bindings`。
4. Gateway 收到私聊消息后调用 `/api/integrations/wechat/process`。API 对消息去重，
   维护 `wechat_drafts`，使用模型提取订阅字段，并只在用户回复“确认”后创建订阅。
5. “哪些订阅快到期”等查询跳过模型，直接返回当前续费周期已生成提醒的订阅。

二维码会话五分钟过期。微信账号 token、cursor 和注册表存放在 `wechat-data` 卷；
绑定、草稿和消息幂等记录存放在 PostgreSQL。

## 通知数据流

Worker 每分钟扫描 `subscriptions` 和 `subscription_reminders`。达到提醒日期后创建
`notifications` 和相应 `notification_deliveries`；到扣款日创建预计账单并推进周期。
微信对话中的“快过期”定义为：有效订阅的当前 `next_billing_date` 已存在对应的
`reminder:<subscription>:<date>:<days>` 通知记录。

## 主要数据表

- 核心：`users`、`subscriptions`、`subscription_reminders`、`bills`
- 通知：`notifications`、`notification_deliveries`、`notification_settings`
- 微信：`wechat_bindings`、`wechat_binding_codes`、`wechat_qr_sessions`、
  `wechat_drafts`、`wechat_messages`、`wechat_rate_limits`

迁移由 API 启动时自动执行，定义位于 `server/migrations/`。
