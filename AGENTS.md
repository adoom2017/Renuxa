@/Users/shendongchun/.codex/RTK.md

# Renuxa 项目约定

- Rust workspace 成员为 `server/`、`im-channel-gateway/`、`src-tauri/`。
- 前端类型、请求、演示数据分别位于 `app/models.ts`、`app/api.ts`、`app/demo-data.ts`；避免继续堆入界面入口。
- Web 业务界面在 `app/`，桌面 Web 入口在 `desktop/`，Tauri 原生壳在 `src-tauri/`。
- 所有容器文件放在 `docker/`；本地构建使用 `docker/compose.build.yml`，预构建镜像使用 `docker/compose.yml`。
- 一键调试命令为 `npm run debug`；启用微信使用 `npm run debug -- --wechat`。
- API 启动时自动运行 `server/migrations/`。新增数据结构必须添加顺序迁移，不修改已发布迁移。
- 微信管理 API 只在容器网络开放，并使用 `WECHAT_GATEWAY_TOKEN` 鉴权。日志和测试输出不得包含模型密钥、网关 token、微信 token 或用户原始图片。
- 提交前至少运行 `cargo test --locked -p renuxa-server -p im-channel-gateway`、`npm test`、`npm run typecheck`、`npm run lint` 和两份 Compose 配置检查。
- 部署与故障处理见 `docs/deployment.md`，系统边界见 `docs/architecture.md`，API 见 `docs/integration-guide.md`，微信流程见 `docs/wechat.md`。
