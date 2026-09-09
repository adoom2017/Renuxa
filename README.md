# 续序 Renuxa

续序是一个服务端驱动的订阅管理应用，包含响应式 Web 客户端、Tauri 桌面壳、Rust API、PostgreSQL 数据库和后台续费任务。

## 本地开发

一键启动本地调试环境并执行 Rust 测试与前端 lint：

```bash
npm run debug
```

只启动服务、不执行测试：`npm run debug -- --no-tests`。按 `Ctrl-C` 会自动停止本次启动的容器。
调试脚本使用根目录 `.env` 中的 `RENUXA_WEB_PORT` 和 `RENUXA_API_PORT`，默认分别为 `3000` 和 `8081`。也可以运行 `RENUXA_WEB_PORT=3001 RENUXA_API_PORT=8082 npm run debug` 临时覆盖配置；端口被占用时启动失败，不会自动切换端口。

前端工作台可以独立启动，默认使用浏览器本地持久化的示例数据：

```bash
npm install
npm run dev
```

## Docker Compose 一键启动

Docker Compose 默认启动 Web、API、Worker 和 PostgreSQL。数据库迁移由 API 启动时自动执行。

### macOS（Colima）

安装 Docker CLI、Compose、Buildx 和 Colima：

```bash
brew install docker docker-buildx docker-compose colima
colima start --cpu 4 --memory 6
```

如果 Docker 找不到 Homebrew 安装的 Compose 或 Buildx 插件，将插件目录加入 `~/.docker/config.json`：

```json
{
  "currentContext": "colima",
  "cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]
}
```

在项目目录启动全部服务：

```bash
cp .env.example .env
docker compose --env-file .env -f docker/compose.yml pull
docker compose --env-file .env -f docker/compose.yml up -d
```

默认的 `docker/compose.yml` 使用 Docker Hub 预构建镜像，不会在服务器上重新编译。
如果需要在本机编译镜像（例如修改了 Rust 或前端代码），使用本地构建文件：

```bash
docker compose --env-file .env -f docker/compose.build.yml up --build -d
```

### Linux 服务器（Docker Engine）

安装 Docker Engine 和 Docker Compose Plugin。以 Ubuntu/Debian 为例，建议按照 [Docker 官方安装文档](https://docs.docker.com/engine/install/) 配置软件源，然后确认以下命令可用：

```bash
docker version
docker compose version
```

克隆仓库后创建环境配置，至少替换 `JWT_SECRET`：

```bash
cp .env.example .env
```

完整环境变量及本地端口配置见 `.env.example`。

发布的 Web 镜像会通过容器网络将同源 `/api` 请求代理到 API 服务，因此部署服务器不需要根据 IP 地址重新编译前端。至少在 `.env` 中替换 `JWT_SECRET`：

```dotenv
JWT_SECRET=replace-with-a-long-random-secret
```

拉取指定版本的预构建镜像并启动：

```bash
docker compose --env-file .env -f docker/compose.yml pull
docker compose --env-file .env -f docker/compose.yml up -d
```

如需本地构建而不是拉取镜像：

```bash
docker compose --env-file .env -f docker/compose.build.yml up --build -d
```

默认部署版本为 `0.1.2`。升级时可在 `.env` 中设置 `RENUXA_VERSION`，再重新拉取并启动。

### 发布 Docker 镜像

Docker Hub 镜像由 GitHub Actions 构建，本地不需要保留 AMD64 虚拟机或手动运行多架构构建。首次使用前，在 GitHub 仓库的 `Settings → Secrets and variables → Actions` 中添加：

| Secret | 内容 |
| --- | --- |
| `DOCKERHUB_USERNAME` | Docker Hub 用户名 `adoom2018` |
| `DOCKERHUB_TOKEN` | Docker Hub 中创建的具有 Read & Write 权限的 Access Token |

发布版本时创建并推送语义化版本标签：

```bash
git tag v0.1.1
git push origin v0.1.1
```

工作流会并行构建 `adoom2018/renuxa-web` 和 `adoom2018/renuxa-server`，分别发布 `linux/amd64`、`linux/arm64` 的 `0.1.1` 与 `latest` 标签。也可以在 GitHub 的 Actions 页面选择 `Publish Docker images`，手动输入版本号运行。

正式的 `docker/compose.yml` 只向宿主机暴露 Web 端口 `3000`。API 和 PostgreSQL 仅在 Compose 内部网络通信。生产环境建议通过 HTTPS 反向代理暴露 Web。

本地调试使用 `docker/compose.build.yml`，该配置额外暴露 API 的 `8081` 端口。

### 通知渠道

通知渠道由每位用户登录后在“设置 → 通知”中配置。应用内通知始终启用，Telegram 可以单独开启。

Telegram 需要填写从 BotFather 获取的 Bot Token，以及接收提醒的 Chat ID。每个账户保存独立配置，提醒只会发送到该账户设置的会话。

Bot Token 写入后不会通过读取接口返回到浏览器；留空保存会保留已配置的密钥。

### 常用命令

```bash
# 查看服务状态
docker compose --env-file .env -f docker/compose.yml ps

# 跟踪所有服务日志
docker compose --env-file .env -f docker/compose.yml logs -f

# 停止并移除容器，保留 PostgreSQL 数据卷
docker compose --env-file .env -f docker/compose.yml down

# 拉取镜像并后台启动
docker compose --env-file .env -f docker/compose.yml pull
docker compose --env-file .env -f docker/compose.yml up -d

# 使用本地源码构建并后台启动
docker compose --env-file .env -f docker/compose.build.yml up --build -d
```

服务默认地址：

| 服务 | 本机地址 | 用途 |
| --- | --- | --- |
| Web | `http://127.0.0.1:3000` | Renuxa Web 界面 |
| API（本地构建配置） | `http://127.0.0.1:8081` | Rust API 与健康检查 |

## 桌面开发

微信调试环境：`npm run debug -- --wechat --no-tests`。登录 Web 后，在“设置 → 微信接入”点击“扫码绑定微信”，扫描页面二维码并确认即可关联当前用户。
登录状态保存在 `wechat-data` 卷中。`--wechat-login` 仅供终端管理登录使用。
先按 [微信接入说明](docs/wechat.md) 配置网关凭据和模型。

```bash
npm run desktop:dev
```

## 目录

- `app/`：Web 和 Tauri 共用的产品界面
- `desktop/`：桌面 Web 入口，加载 `app/` 中的共用界面
- `server/`：Axum API、SQLx 迁移与后台 Worker
- `im-channel-gateway/`：微信与 Telegram 消息网关
- `src-tauri/`：macOS/Windows 桌面应用配置
- `docker/`：Dockerfile、Compose、Nginx 和微信网关配置
- `docs/`：部署、架构、接口、微信接入和交接文档

## 生产配置

生产环境必须替换 `JWT_SECRET`、启用 HTTPS，并为数据库配置独立凭据和每日备份。Apple 图标搜索仅保存远程来源 URL；正式发布前应复核相应内容使用条款。
# 微信接入

内置独立微信网关，支持绑定用户、文字或截图提取订阅、追问补全及确认入库。
默认关闭；构建、扫码登录、配置、备份和验收见 [微信部署说明](docs/wechat.md)。
