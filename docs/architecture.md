# 系统架构

## 服务边界

| 目录 / 服务 | 职责 |
| --- | --- |
| `app/` | Web 与桌面端共用的 React 产品界面 |
| `desktop/` | Vite 桌面 Web 入口 |
| `src-tauri/` | Tauri 原生桌面壳 |
| `server/` / API | Axum HTTP API、认证、订阅与微信业务状态机 |
| `server/` / Worker | 生成账单和通知、发送 Telegram 通知、清理过期状态 |
| `im-channel-gateway/` | 微信 iLink 和 Telegram 消息接入、媒体转换与消息转发 |
| PostgreSQL | 用户、订阅、账单、通知、通知设置及微信会话状态 |

Web 通过 Nginx 将同源 `/api` 请求转发到 API。Gateway 通过受 token 保护的
HTTP SSE 接口向 API 投递微信消息；API 通过 Gateway 管理接口创建和轮询登录二维码。

## 代码组织

- `app/renuxa-app.tsx` 组合产品界面，`bills-view.tsx` 展示账单与未来扣款预估；`models.ts` 定义共用类型，`api.ts` 负责
  请求和服务端数据映射，`billing.ts` 负责计费日期，`use-stored-state.ts` 管理本地持久化。
- `app/demo-data.ts` 隔离离线示例数据；`copy.ts` 和 `constants.ts` 保存文案与显示常量。
- `server/src/routes.rs` 组装业务路由，`routes/icons.rs` 封装图标搜索和代理；
  `subscriptions.rs` 共享订阅校验与入库逻辑，`wechat.rs` 实现微信状态机。
- 网关保留上游可配置的渠道与 agent 适配器；Renuxa 部署只启用微信和 HTTP SSE。
- `src-tauri/gen/schemas/` 是原生构建生成的 schema，不纳入版本控制。

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

## 账单展示

订阅可填写 `start_date`。服务端按用户时区，将开始日期到当天（含）的周期按当前
订阅金额补记为已支付，并计算第一个未来扣款日；后续由 Worker 在扣款日自动记账。
月末以开始日为锚点，闰年和季度、周、日及周期倍数均沿同一规则计算。一次性订阅
只记一笔，记账后结束。历史金额按录入时金额计算，不推断以往调价或暂停区间。

迁移 `0006` 给旧订阅保留空开始日期及原续费安排；用户编辑补填后才补账。
无开始日期的旧客户端仍按原方式在扣款日生成待确认账单。账单 `source` 区分
自动周期记账（schedule）、旧续费账单（renewal）与人工改过状态的账单（manual）。
修改开始日期或周期时，仅删除不再属于该周期的自动账单，补齐缺失账单；相同日期
已有账单金额和人工状态保留。仅修改价格不改写历史账单。

账单页按账单日统计本年已支付记录；待确认数量包含逾期账单。API 按稳定顺序每页
返回 200 条，前端用 `offset` 加载全部记录后再统计。未来 30 天预估独立展示，
已入账的同订阅同日期不会重复展示，预估不能直接确认支付。
汇率同步由 `server/src/exchange_rates.rs` 负责，使用 Frankfurter v1 的 `.dev` 地址；
校验完整快照后事务写入，失败不会覆盖缓存。
未来扣款预估使用 `/api/exchange-rates` 的最新参考汇率。Worker 按账单日期查询历史
汇率（非交易日使用供应商返回的前一个交易日），补齐 `bills.base_amount`、
`base_currency`、`exchange_rate`、`exchange_rate_date`；已有完整折算结果不覆盖。
原币金额、日期和支付状态不变。每轮最多处理 5 个日期、总计最多 30 秒，后续轮次
继续处理剩余记录。账单接口附带该汇率日的 `reference_rates`，页面切换显示货币
也使用对应历史汇率。缺少历史汇率时保留原币金额并提示待同步，不使用最新汇率替代。账户首次加载成功前不显示本地缓存
或示例账单，刷新失败会提示错误。

## 主要数据表

- 核心：`users`、`subscriptions`、`subscription_reminders`、`bills`
- 通知：`notifications`、`notification_deliveries`、`notification_settings`
- 汇率：`exchange_rates`
- 微信：`wechat_bindings`、`wechat_binding_codes`、`wechat_qr_sessions`、
  `wechat_drafts`、`wechat_messages`、`wechat_rate_limits`

迁移由 API 启动时自动执行，定义位于 `server/migrations/`。
旧数据库可能保留已停用的邮件配置列与历史投递记录；运行时代码不会读取或写入它们。
