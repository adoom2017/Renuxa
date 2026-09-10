# API 接入指南

API 默认监听容器内 `8080`。本地构建 Compose 默认发布到
`http://127.0.0.1:8081`，Web 部署通过同源 `/api` 访问。账户业务接口使用 `Authorization: Bearer <JWT>`。
注册、登录、健康检查和图标搜索/代理无需 JWT；微信消息投递使用网关 token。

## HTTP 路由

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `POST` | `/api/auth/register` | 注册并获取 JWT |
| `POST` | `/api/auth/login` | 登录并获取 JWT |
| `GET, POST` | `/api/subscriptions` | 查询或创建订阅 |
| `PATCH, DELETE` | `/api/subscriptions/{id}` | 编辑订阅详情、更新状态或归档订阅 |
| `GET` | `/api/bills` | 查询账单 |
| `PATCH` | `/api/bills/{id}` | 更新账单状态 |
| `GET` | `/api/notifications` | 查询通知 |
| `POST` | `/api/notifications/{id}/read` | 标记通知已读 |
| `GET, PUT` | `/api/notification-settings` | 获取或更新通知渠道 |
| `GET` | `/api/icons/search` | 搜索美国区 App Store 订阅图标（固定 `country=us`） |
| `GET` | `/api/icons/image` | 代理允许来源的图标 |
| `GET` | `/api/exchange-rates` | 查询汇率 |
| `GET` | `/api/dashboard` | 获取仪表盘汇总 |
| `POST` | `/api/integrations/wechat/qrcode` | 为当前用户创建微信二维码会话 |
| `GET` | `/api/integrations/wechat/qrcode/status` | 查询当前用户扫码状态并完成绑定 |
| `GET, DELETE` | `/api/integrations/wechat/binding` | 查询或解除微信绑定 |
| `POST` | `/api/integrations/wechat/binding-code` | 兼容旧版文字绑定码 |
| `POST` | `/api/integrations/wechat/process` | Gateway 投递微信消息，使用网关 token |

健康检查为 `GET /health`。错误响应格式为：

```json
{"error":{"code":422,"message":"请求内容无效: ..."}}
```

## 微信二维码

创建二维码时提交用户时区：

```http
POST /api/integrations/wechat/qrcode
Authorization: Bearer <JWT>
Content-Type: application/json

{"timezone":"Asia/Shanghai"}
```

响应包含 Data URL 格式的 PNG 和有效秒数。客户端每两秒调用状态接口，直到
`confirmed`、`expired`、`idle` 或 `stale`。会话只允许创建它的 Renuxa 用户查询。

Gateway 管理接口由 API 在容器网络内调用，所有请求使用
`WECHAT_GATEWAY_TOKEN`。该 token 至少 32 字符，且必须与 Gateway 配置一致。
