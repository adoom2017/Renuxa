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
| `GET` | `/api/bills` | 分页查询账单（`offset` 默认 0，每页最多 200 条） |
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

## 订阅开始日期与记账

创建及完整编辑订阅推荐提交 `start_date`，不再要求客户端计算下次续费日：

```json
{"name":"示例服务","amount":"30.00","currency":"CNY","cadence_unit":"month","cadence_interval":1,"start_date":"2026-01-31"}
```

开始日期到用户当地当天的每个周期记为已支付，下次扣款日自动计算。例如当地日期为
2026-03-10，上例生成 1 月 31 日和 2 月 28 日两笔账单，下次扣款日为 3 月 31 日。
响应同时包含 `start_date` 和服务端计算的 `next_billing_date`。将来的开始日期不会
提前生成账单。开始日期必填于新版 Web 表单；旧 API 的 `next_billing_date` 仍兼容，
仅提交该字段不会推断历史付款。两个字段同时存在时以 `start_date` 为计算依据。

历史按当前金额补记，重复保存不会重复入账。修改开始日期或周期会调整自动生成的
账单，人工修改过状态的账单保留；仅修改金额不重写历史。暂停或取消后 Worker
不再续记，恢复后按保留的续费进度继续处理。

`GET /api/bills?offset=0` 返回前 200 条，继续请求 `offset=200` 等，直到返回不足
200 条。顺序为账单日倒序、ID 倒序。统计应基于完整记录，不能只汇总第一页。
账单响应中的 `base_amount`、`base_currency` 保存历史折算结果，`exchange_rate_date`
是实际参考交易日；`reference_rates` 为该日以 EUR 为基准的汇率映射。Worker 在后台
补齐这些字段，未完成时相关字段为 null、映射为空。已入账金额应使用这些历史值，
不能用最新参考汇率重新计算；原币与目标币相同则直接使用原币金额。

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
