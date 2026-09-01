# 续序 Renuxa

续序是一个服务端驱动的订阅管理应用，包含响应式 Web 客户端、Tauri 桌面壳、Rust API、PostgreSQL 数据库和后台续费任务。

## 本地开发

前端工作台可以独立启动，默认使用浏览器本地持久化的示例数据：

```bash
npm install
npm run dev
```

### Colima 服务端环境

推荐使用 Colima 运行完整服务端。首次使用先安装 Docker 插件：

```bash
brew install docker-buildx docker-compose
```

Homebrew 的 Docker 插件目录需要加入 `~/.docker/config.json`：

```json
{
  "currentContext": "colima",
  "cliPluginsExtraDirs": ["/opt/homebrew/lib/docker/cli-plugins"]
}
```

随后使用 Docker Compose 一键启动完整服务（Web、API、Worker、PostgreSQL 和 Mailpit）：

```bash
docker compose up --build
```

Web 默认监听 `http://127.0.0.1:3000`，API 默认监听 `http://127.0.0.1:8080`，Mailpit 在 `http://127.0.0.1:8025`。数据库迁移由 API 启动时自动执行。

可通过环境变量覆盖 Colima 资源，或停止项目容器：

```bash
docker compose up --build -d
docker compose down
```

桌面开发模式：

```bash
npm run desktop:dev
```

## 目录

- `app/`：Web 和 Tauri 共用的产品界面
- `server/`：Axum API、SQLx 迁移与后台 Worker
- `src-tauri/`：macOS/Windows 桌面应用配置
- `docker-compose.yml`：PostgreSQL、API、Worker 与开发邮件服务

## 生产配置

生产环境必须替换 `JWT_SECRET`、启用 HTTPS，并为数据库配置独立凭据和每日备份。Apple 图标搜索仅保存远程来源 URL；正式发布前应复核相应内容使用条款。
