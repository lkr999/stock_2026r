#!/usr/bin/env bash
# Launch backend (FastAPI) + frontend (SvelteKit) for local development.
# Usage: ./run_dev.sh   (requires .env EBEST_APP_KEY / EBEST_APP_SECRET)
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

# Backend (requires Python 3.13+)
cd "$ROOT/backend"
PY="$(command -v python3.13 || command -v python3)"
if [ ! -d .venv ]; then
  "$PY" -m venv .venv
  .venv/bin/pip install -q --upgrade pip
  .venv/bin/pip install -q -r requirements.txt
fi
.venv/bin/python -m uvicorn app.main:app --port 8000 &
BACKEND_PID=$!

# Frontend
cd "$ROOT/frontend"
[ -d node_modules ] || npm install
npm run dev -- --port 5173 &
FRONTEND_PID=$!

echo "Backend  : http://localhost:8000/docs"
echo "Frontend : http://localhost:5173"
wait
