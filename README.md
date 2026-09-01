# 续序 Renuxa

续序是一个服务端驱动的订阅管理应用，包含响应式 Web 客户端、Tauri 桌面壳、Rust API、PostgreSQL 数据库和后台续费任务。

## 本地开发

前端工作台可以独立启动，默认使用浏览器本地持久化的示例数据：

```bash
npm install
npm run dev
```

## Docker Compose 一键启动

Docker Compose 会启动 Web、API、Worker、PostgreSQL 和 Mailpit。数据库迁移由 API 启动时自动执行。

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
docker compose up --build -d
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

如果只在服务器本机浏览器访问，`.env.example` 中的默认 `VITE_API_URL` 可以直接使用。如果从其他电脑访问服务器，必须将它改为浏览器可访问的地址，例如：

```dotenv
JWT_SECRET=replace-with-a-long-random-secret
VITE_API_URL=http://192.0.2.10:8080/api
```

`VITE_API_URL` 会在 Web 镜像构建时写入前端，因此修改后需要重新构建：

```bash
docker compose up --build -d
```

服务器防火墙至少需要允许 Web 端口 `3000` 和 API 端口 `8080`。Mailpit 的 `8025` 和 SMTP 的 `1025` 仅用于开发调试，不应直接暴露到公网。生产环境建议通过 HTTPS 反向代理统一暴露 Web 和 API。

### 常用命令

```bash
# 查看服务状态
docker compose ps

# 跟踪所有服务日志
docker compose logs -f

# 停止并移除容器，保留 PostgreSQL 数据卷
docker compose down

# 重新构建并后台启动
docker compose up --build -d
```

服务默认地址：

| 服务 | 本机地址 | 用途 |
| --- | --- | --- |
| Web | `http://127.0.0.1:3000` | Renuxa Web 界面 |
| API | `http://127.0.0.1:8080` | Rust API 与健康检查 |
| Mailpit | `http://127.0.0.1:8025` | 查看开发环境邮件 |

## 桌面开发

```bash
npm run desktop:dev
```

## 目录

- `app/`：Web 和 Tauri 共用的产品界面
- `server/`：Axum API、SQLx 迁移与后台 Worker
- `src-tauri/`：macOS/Windows 桌面应用配置
- `docker-compose.yml`：Web、PostgreSQL、API、Worker 与开发邮件服务

## 生产配置

生产环境必须替换 `JWT_SECRET`、启用 HTTPS，并为数据库配置独立凭据和每日备份。Apple 图标搜索仅保存远程来源 URL；正式发布前应复核相应内容使用条款。
