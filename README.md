# 续序 Renuxa

续序是一个订阅管理应用，包含 Web 界面、Tauri 桌面壳、Rust API、PostgreSQL 和后台续费任务。
订阅填写开始日期后，按扣费周期自动补记历史账单并计算下次续费。
支持订阅与账单管理、应用内和 Telegram 通知，以及微信文字、截图录入。

## 本地开发

需要 Node.js >= 22.13、Rust 和 Docker Compose。先安装依赖并创建本地配置：

```bash
npm ci
cp .env.example .env
```

修改 `.env` 中的 `JWT_SECRET`，然后启动完整调试环境：

```bash
npm run debug
```

脚本构建并启动 Web、API、Worker 和 PostgreSQL，等待健康检查，运行 Rust 测试、
前端测试、类型检查和 lint，随后跟踪日志。`Ctrl-C` 停止容器并保留数据卷。
默认 Web 端口为 `3000`、API 为 `8081`；可通过 `RENUXA_WEB_PORT`、
`RENUXA_API_PORT` 覆盖。端口被占用时不会自动切换。

- 跳过检查：`npm run debug -- --no-tests`
- 启用微信：配置网关凭据和模型后运行 `npm run debug -- --wechat`
- 仅开发前端：`npm run dev`，默认使用浏览器本地保存的示例数据
- 前端连接 API：`VITE_API_URL=http://127.0.0.1:8081 npm run dev`
- 桌面开发：`npm run desktop:dev`（需要对应平台的 Tauri 构建依赖）

## 部署

预构建镜像部署：

```bash
docker compose --env-file .env -f docker/compose.yml pull
docker compose --env-file .env -f docker/compose.yml up -d
```

从源码构建：

```bash
docker compose --env-file .env -f docker/compose.build.yml up --build -d --wait
```

Compose 默认启动微信网关；配置要求见 [微信接入](docs/wechat.md)。
调试脚本则仅在传入 `--wechat` 或 `--wechat-login` 时启用微信。
生产配置仅发布 Web 端口，通过 Nginx 代理同源 `/api` 请求；数据库迁移由 API 自动执行。
生产环境需替换 JWT 密钥、配置 HTTPS、数据库凭据与备份。

## 代码结构

| 路径 | 职责 |
| --- | --- |
| `app/` | 共用 React 界面、领域类型、API 客户端、计费逻辑和本地演示数据 |
| `desktop/` | 共用界面的独立 Vite 入口，供 Docker Web 与桌面打包使用 |
| `src-tauri/` | Tauri 原生桌面壳 |
| `server/` | Axum API、SQLx 顺序迁移与后台 Worker |
| `im-channel-gateway/` | 微信与 Telegram 消息网关，来源和许可见目录内说明 |
| `docker/` | 镜像构建、Compose、Nginx 与微信配置 |
| `scripts/` | 本地调试脚本 |

Web 开发和 `npm run build` 使用 Vinext；Docker 与 Tauri 发布使用
`npm run build:desktop-web` 生成静态界面，不需要私有托管配置。

## 检查与文档

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

- [系统架构](docs/architecture.md)：模块边界、数据流与数据表
- [部署与运维](docs/deployment.md)：环境、升级、镜像发布和排障
- [API 接入](docs/integration-guide.md)：路由、认证与响应格式
- [微信接入](docs/wechat.md)：配置、绑定、录入、备份与验收
