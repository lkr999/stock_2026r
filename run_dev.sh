#!/usr/bin/env bash
# Launch backend (Rust/axum) + frontend (SvelteKit) for local development.
# Usage: ./run_dev.sh   (requires backend/.env EBEST_APP_KEY / EBEST_APP_SECRET)
# 모든 데이터는 eBest API 에서만 가져옵니다. 키 미설정 시 데이터 조회는 인증 오류를 반환합니다.
set -e
ROOT="$(cd "$(dirname "$0")" && pwd)"

BACKEND_PID=""
FRONTEND_PID=""
cleanup() {
  [ -n "$BACKEND_PID" ]  && kill "$BACKEND_PID"  2>/dev/null || true
  [ -n "$FRONTEND_PID" ] && kill "$FRONTEND_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Backend (requires Rust / cargo)
cd "$ROOT/backend"
cargo run --release &
BACKEND_PID=$!

# Frontend (requires Node 18+)
cd "$ROOT/frontend"
[ -d node_modules ] || npm install
npm run dev -- --port 7777 &
FRONTEND_PID=$!

echo "Backend  : http://localhost:7001/api/health"
echo "Frontend : http://localhost:7777"
wait
