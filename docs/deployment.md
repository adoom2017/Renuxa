# 部署与运维

## 环境准备

需要 Docker Engine 与 Compose Plugin。macOS 可安装 Docker CLI 和 Colima：

```bash
brew install docker docker-buildx docker-compose colima
colima start --cpu 4 --memory 6
```

Homebrew 插件未被发现时，将 `/opt/homebrew/lib/docker/cli-plugins` 加入
`~/.docker/config.json` 的 `cliPluginsExtraDirs`。Linux 安装方式见
[Docker 官方文档](https://docs.docker.com/engine/install/)。

## 配置

从仓库根目录创建配置，并至少替换 `JWT_SECRET`：

```bash
cp .env.example .env
```

本地构建端口由 `RENUXA_WEB_PORT` 和 `RENUXA_API_PORT` 控制，默认分别为
`3000` 和 `8081`。Compose 默认启用微信并启动 Gateway；微信功能还需要
`WECHAT_GATEWAY_TOKEN`、`WECHAT_MODEL_URL`、`WECHAT_MODEL_NAME` 和
`WECHAT_MODEL_API_KEY`。Docker 默认启用中英文 OCR，可通过 `WECHAT_OCR_ENABLED`
和 `WECHAT_OCR_LANG` 调整。需要关闭微信时可设置 `WECHAT_ENABLED=false`。
非 Compose 部署还需设置 `WECHAT_GATEWAY_URL`；
`WECHAT_GATEWAY_ID` 默认且应稳定保持为 `renuxa-wechat`。详见 [微信接入](wechat.md)。

## 本地调试

构建、启动、等待健康检查、执行测试并跟踪日志：

```bash
npm run debug
```

调试脚本默认关闭微信 API 且不启动网关。只启动服务使用 `npm run debug -- --no-tests`；启用微信网关使用
`npm run debug -- --wechat --no-tests`。按 `Ctrl-C` 停止容器并保留数据卷。
固定端口被占用时脚本会失败，不会自动改用其他端口。

## 预构建镜像

正式 Compose 默认使用 Docker Hub 的 `latest` 标签；在 `.env` 中设置
`RENUXA_VERSION` 可指定版本。每次升级先拉取镜像：

```bash
docker compose --env-file .env -f docker/compose.yml pull
docker compose --env-file .env -f docker/compose.yml up -d
```

正式 Compose 只发布 Web 的 `3000` 端口。API、PostgreSQL 和微信网关仅在
容器网络中通信。部署时应在 Web 前配置 HTTPS 反向代理。

## 本地镜像

```bash
docker compose --env-file .env -f docker/compose.build.yml up --build -d --wait
```

该配置发布 Web 和 API 端口。`--wait` 会等到 Web、API 和 PostgreSQL 健康后再返回；
不使用 `--wait` 时，命令返回后的前几十秒内服务仍可能处于启动阶段。

## 通知配置

用户登录后在“设置 → 通知”配置 Telegram 的 Bot Token 和 Chat ID。
应用内通知始终启用；Telegram 凭据按账户独立保存，读取接口不会返回 Bot Token，
保存时留空会保留已有密钥。Worker 必须运行，才会生成续费提醒和发送通知。

## 汇率同步排障

Worker 从 `https://api.frankfurter.dev/v1/latest?from=EUR` 获取参考汇率，15 秒超时。
旧 `.app` 地址会返回 301；应用的共享 HTTP 客户端不跟随重定向。
同步失败时 Worker 记录 `exchange rate sync failed` 并保留上次成功的数据。
供应商按交易日发布，页面显示上一个交易日的日期是正常情况。

在本地容器中验证上游连接：

```bash
docker compose -f docker/compose.build.yml exec worker curl --fail --silent --show-error --max-time 15 'https://api.frankfurter.dev/v1/latest?from=EUR'
```

上游恢复后 Worker 会继续同步，并自动补齐历史账单的折算金额。补齐按账单日查询
历史快照，每轮最多 5 个日期；失败或超时留待后续重试。日志
`historical bill exchange rates updated` 表示已有账单完成补齐。账单页点击
“刷新账单”可读取更新后的结果。尚未填写订阅开始日期的项目不会因此自动生成历史账单。

## 订阅开始日期升级

升级 API 会自动应用 `0006_subscription_start_date.sql`，新增订阅开始日期和
账单来源列。需一并更新 API、Worker 和 Web。旧订阅不会自动猜测开始日期，
请在“我的订阅”编辑补填；保存后会按当前金额和周期补记历史已支付账单。

## 日常操作

```bash
docker compose --env-file .env -f docker/compose.yml ps
docker compose --env-file .env -f docker/compose.yml logs -f
docker compose --env-file .env -f docker/compose.yml down
```

本地构建环境将文件名替换为 `docker/compose.build.yml`。不要执行
`docker compose down -v`，除非明确要删除 PostgreSQL 和微信登录数据。

微信会话出现 `ret=-14: session timeout` 时，重新扫码刷新登录凭据。若需要只清理
微信状态，先删除 gateway 容器，再删除 Compose 的 `wechat-data` 卷；不要删除
`postgres-data`。

## 发布镜像

`.github/workflows/docker-images.yml` 从 `docker/Dockerfile.web` 和
`docker/Dockerfile.server` 构建 `linux/amd64`、`linux/arm64` 镜像。推送形如
`v0.1.2` 的标签，或在 GitHub Actions 手动输入版本即可发布。
仓库 Actions Secrets 需配置 `DOCKERHUB_USERNAME` 和具备写入权限的
`DOCKERHUB_TOKEN`。镜像名为 `adoom2018/renuxa-web` 和
`adoom2018/renuxa-server`，标签发布会同时更新版本标签与 `latest`。

## 验证

```bash
cargo test --locked -p renuxa-server -p im-channel-gateway
npm test
npm run typecheck
npm run lint
npm run build
npm run build:desktop-web
docker compose -f docker/compose.yml config --quiet
docker compose -f docker/compose.build.yml config --quiet
```
