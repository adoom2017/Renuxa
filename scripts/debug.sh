#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE=(docker compose --project-directory "$ROOT_DIR/docker" -f "$ROOT_DIR/docker/compose.build.yml")
if [[ -f "$ROOT_DIR/.env" ]]; then
  COMPOSE+=(--env-file "$ROOT_DIR/.env")
fi
RUN_TESTS=1
WECHAT=0
WECHAT_LOGIN=0

for arg in "$@"; do
  case "$arg" in
    --no-tests) RUN_TESTS=0 ;;
    --wechat) WECHAT=1 ;;
    --wechat-login) WECHAT=1; WECHAT_LOGIN=1 ;;
    *) echo "Unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if (( WECHAT )); then
  export WECHAT_ENABLED=true
else
  export WECHAT_ENABLED=false
fi

cleanup() { "${COMPOSE[@]}" down --remove-orphans >/dev/null 2>&1 || true; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cd "$ROOT_DIR"
# Compose reads configured ports from the environment or the root .env file.
if (( WECHAT_LOGIN )); then
  "${COMPOSE[@]}" stop gateway
fi
if ! "${COMPOSE[@]}" up --build -d --wait --wait-timeout 180 web api worker postgres; then
  "${COMPOSE[@]}" logs --tail 100
  exit 1
fi
WEB_ADDRESS="$("${COMPOSE[@]}" port web 80)"
API_ADDRESS="$("${COMPOSE[@]}" port api 8080)"
echo "Web: http://localhost:${WEB_ADDRESS##*:}"
echo "API: http://localhost:${API_ADDRESS##*:}"

if (( WECHAT_LOGIN )); then
  echo "Scan the gateway QR code with WeChat and confirm the login."
  "${COMPOSE[@]}" run --rm --no-deps gateway im-channel-gateway --config /etc/renuxa/wechat.toml login wechat
fi
if (( WECHAT )); then
  "${COMPOSE[@]}" up --build -d --wait --wait-timeout 180 gateway
fi

if (( RUN_TESTS )); then
  cargo test --locked -p renuxa-server -p im-channel-gateway
  npm test
  npm run typecheck
  npm run lint
fi

echo "Debug services are ready. Press Ctrl-C to stop them."
"${COMPOSE[@]}" logs -f
