# 部署与运维

## 配置

从仓库根目录创建配置，并至少替换 `JWT_SECRET`：

```bash
cp .env.example .env
```

本地构建端口由 `RENUXA_WEB_PORT` 和 `RENUXA_API_PORT` 控制，默认分别为
`3000` 和 `8081`。微信功能还需要 `WECHAT_ENABLED`、
`WECHAT_GATEWAY_TOKEN`、`WECHAT_MODEL_URL`、`WECHAT_MODEL_NAME` 和
`WECHAT_MODEL_API_KEY`。非 Compose 部署还需设置 `WECHAT_GATEWAY_URL`；
`WECHAT_GATEWAY_ID` 默认且应稳定保持为 `renuxa-wechat`。详见 [微信接入](wechat.md)。

## 本地调试

构建、启动、等待健康检查、执行测试并跟踪日志：

```bash
npm run debug
```

只启动服务使用 `npm run debug -- --no-tests`；启用微信网关使用
`npm run debug -- --wechat --no-tests`。按 `Ctrl-C` 停止容器并保留数据卷。
固定端口被占用时脚本会失败，不会自动改用其他端口。

## 预构建镜像

```bash
docker compose --env-file .env -f docker/compose.yml pull
docker compose --env-file .env -f docker/compose.yml up -d
```

正式 Compose 只发布 Web 的 `3000` 端口。API、PostgreSQL、微信网关和可选
Mailpit 仅在容器网络中通信。部署时应在 Web 前配置 HTTPS 反向代理。

## 本地镜像

```bash
docker compose --env-file .env -f docker/compose.build.yml up --build -d
```

该配置发布 Web、API 端口；通过 `mail` profile 启用 Mailpit：

```bash
docker compose --env-file .env -f docker/compose.build.yml --profile mail up --build -d
```

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

## 验证

```bash
cargo test --locked -p renuxa-server -p im-channel-gateway
npx tsc --noEmit
npm run lint
npm run build
docker compose -f docker/compose.yml config --quiet
docker compose -f docker/compose.build.yml --profile wechat config --quiet
```
