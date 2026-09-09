# 微信订阅录入

网关源码位于 `im-channel-gateway/`，来源提交和许可见该目录的
`UPSTREAM.md`、`LICENSE`。不需要原 rust-agent 目录。
API、Worker、Gateway 使用同一版本服务端镜像，网关独立运行。

## 配置与启动

本地调试使用以下命令启动网关，在 Web 的“设置 → 微信接入 → 扫码绑定微信”获取二维码：

```sh
npm run debug -- --wechat --no-tests
```

先完成下方环境变量配置。脚本会启用微信 API；微信扫码确认后，页面自动关联当前 Renuxa 用户，不需要发送绑定码。
二维码五分钟有效，刷新间隔三十秒。微信未返回扫码用户身份时不会建立绑定。
退出调试会停止服务，保留登录数据卷。`--wechat-login` 仍保留为终端管理入口，不会关联 Web 用户。

在部署环境配置以下变量（不提交密钥）：

```dotenv
WECHAT_ENABLED=true
WECHAT_GATEWAY_TOKEN=<至少32字符的随机服务凭据>
WECHAT_MODEL_URL=https://your-provider.example/v1/chat/completions
WECHAT_MODEL_NAME=<支持JSON输出的模型名>
WECHAT_MODEL_API_KEY=<模型密钥>
WECHAT_OCR_ENABLED=true
WECHAT_OCR_LANG=chi_sim+eng
```

Docker 默认使用容器内 Tesseract 识别中英文图片文字，再交给模型提取订阅字段；
图片不写盘，也不会发送到外部模型。`WECHAT_OCR_ENABLED=false` 时会改为把
`image_url` 内联 Base64 图片直接交给模型，此时模型必须支持视觉输入。
`WECHAT_MODEL_URL` 是完整 Chat Completions 地址，模型服务必须支持
`response_format: json_object`。
请求超时 45 秒，不成功或非法 JSON 不覆盖已有草稿。
网关身份由 API 的 `WECHAT_GATEWAY_ID` 指定，默认 `renuxa-wechat`；
更换此身份会使旧身份下的绑定无法使用，正常升级不要更改。

默认配置在镜像 `/etc/renuxa/wechat.toml`，管理接口只监听容器
`0.0.0.0:18765`，仅容器网络可访问，未发布宿主机端口；所有管理请求使用 `WECHAT_GATEWAY_TOKEN` 验证。API 通过 `WECHAT_GATEWAY_URL` 访问网关，Compose 默认 `http://gateway:18765`；只启用微信和 http_sse。

首次部署建议使用本仓库构建（现有旧发布镜像不包含此功能）：

```sh
docker compose --env-file .env -f docker/compose.build.yml up --build -d
docker compose --env-file .env -f docker/compose.build.yml run --rm gateway im-channel-gateway --config /etc/renuxa/wechat.toml login wechat
docker compose --env-file .env -f docker/compose.build.yml up -d gateway
```

扫码登录命令会在终端显示二维码，并写入独立 `wechat-data` 卷。
登录 CLI 完成后再启动 gateway，避免并发修改账号注册文件。
镜像发布后，改用 `-f docker/compose.yml` 拉取预构建镜像；默认标签为
`latest`，也可通过 `RENUXA_VERSION` 指定版本。现有 amd64/arm64 发布流程无需变更。

如需生成通用配置，可运行：

```sh
cargo run -p im-channel-gateway -- --config /tmp/gateway.example.toml init
```

该通用示例默认关闭所有渠道。Renuxa 专用模板为 `docker/wechat.toml`，
本机运行时将模板的 API 地址及数据目录改为本机对应路径。
`WECHAT_GATEWAY_TOKEN` 会覆盖配置中的 `agent.bearer_token`。

## 绑定与录入

登录 Renuxa，在设置中的“微信接入”点击“扫码绑定微信”，使用微信扫描页面二维码并确认。
页面轮询扫码结果并绑定微信返回的用户身份与网关账号。会话归属当前登录用户，其他用户不能查询该会话。
旧版绑定码 API 保留兼容用途；发送者每分钟最多处理二十次请求。

发送一项订阅文字或截图，信息不全时补充回答。完整信息将展示预览，
回复“确认”才创建。发送修改信息后会重新预览；“取消”清除草稿。
也可以发送“哪些订阅快到期了”“查看到期提醒”或“哪些订阅发了通知”等
自然语言查询。网关会返回当前续费周期已经生成过提醒的有效订阅，包括名称、
金额、币种、扣款日期和剩余天数；历史周期通知不会计入结果。
发现相同有效订阅时须回复“仍然添加”。草稿二十四小时后过期。
解绑立即删除草稿；过期草稿由 Worker 定期清理。
用户时区取自服务端 `users.timezone`（默认 Asia/Shanghai）；
页面生成绑定码时会将通用设置的时区同步到服务端，供相对日期解析使用。
以后更改本设备显示时区不会自动改变已经绑定的录入时区。

仅支持私聊文字及 JPEG、PNG、WebP；每条最多三张、单图八 MB、
一千六百万像素、单边最多 8192。图片从固定微信 CDN 下载，拒绝跳转，
校验并解密后内联传送，不将原图写盘。服务端再次验证实际图片内容。
群聊、语音、网页抓取、批量导入、编辑删除和主动微信提醒不支持。

消息按网关身份、机器人账号、发送者和稳定消息 ID 去重。
事务内一起提交草稿确认、订阅及成功回复。确认请求重试返回原结果。
消息结果保留用于幂等性，不保存原始请求或图片；数据库备份包含已提取
订阅字段和回复，按订阅数据同等保护。请勿开启 HTTP 请求体调试日志。

## 重启、重新登录与备份

每个账号数据卷只允许一个运行中的 gateway，不能横向扩容该服务。
API 和 gateway 可分别重启；账号、token、cursor 在卷内，草稿在 PostgreSQL。

```sh
docker compose --env-file .env -f docker/compose.yml stop gateway
docker compose --env-file .env -f docker/compose.yml run --rm gateway im-channel-gateway --config /etc/renuxa/wechat.toml login wechat
docker compose --env-file .env -f docker/compose.yml up -d gateway
```

重新扫码应使用同一机器人微信账号。切换机器人账号前需解除旧绑定，
并使用新的独立数据卷。首次版本限制一个机器人账号，但可绑定多个用户。
备份时停止 gateway，归档完整 `wechat-data` 卷，另行备份 PostgreSQL；
恢复时同时恢复账号卷和数据库，不要只恢复 token 或 cursor。
不要运行 `docker compose down -v`，它会删除持久化卷。
关闭 gateway 并将 `WECHAT_ENABLED=false` 后，现有 Web 订阅功能仍可使用。

## 验证

```sh
cargo test --locked -p renuxa-server -p im-channel-gateway
npx tsc --noEmit
node --test app/billing.test.ts
npm run lint
npm run build
docker compose --env-file .env -f docker/compose.yml config --quiet
docker compose --env-file .env -f docker/compose.build.yml config --quiet
# 使用可创建临时测试数据库的本地 PostgreSQL：
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/postgres cargo test -p renuxa-server --test wechat_flow -- --ignored
# 构建并运行隔离容器验收（使用本机 55440 端口，自动清理测试容器和卷）：
docker build -f docker/Dockerfile.server -t renuxa-server:wechat-verification .
node server/tests/container-smoke.mjs
```

上线前必须使用实际模型和微信扫码验收文字、截图、截图后补文字、
歧义币种、相对日期、月末日期、连续截图和确认，以及网关/API 重启。
自动化 mock 模型测试不能证明模型提供商的实际识图质量。

2026-09-08 本地验证：PostgreSQL 17 集成测试通过，覆盖并发确认、重复请求、
连续图片、图片后修改、非法图片及换绑后的旧消息隔离；Linux ARM64 镜像构建
成功。隔离容器验收通过 API 重启后的草稿保留、确认响应丢弃后幂等重试、
网关账号/token/cursor 测试数据保留、单实例锁、关闭网关后订阅 API 可用及
同镜像 Worker 启动。账号持久化采用禁用的模拟账号，未进行真实微信扫码、
真实模型识图或本地 AMD64 镜像构建；发布流程仍配置 amd64/arm64 双架构。
