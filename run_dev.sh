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

# Wait for the backend to actually answer before starting the frontend —
# otherwise Vite's /api proxy hits it before it's bound and the browser (or
# Vite's dev server itself) gets ECONNREFUSED on the first request(s).
echo -n "Waiting for backend "
for i in $(seq 1 60); do
  sleep 1
  if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
    echo ""; echo "오류: 백앤드 프로세스가 종료됨 (cargo run 출력을 확인하세요)." >&2
    exit 1
  fi
  if curl -fsS http://localhost:7001/api/health >/dev/null 2>&1; then
    echo " OK ($i 초)"; break
  fi
  echo -n "."
  [ "$i" -eq 60 ] && { echo ""; echo "오류: 백앤드가 60초 내에 응답하지 않음." >&2; exit 1; }
done

# Frontend (requires Node 18+)
cd "$ROOT/frontend"
[ -d node_modules ] || npm install
npm run dev -- --port 7777 &
FRONTEND_PID=$!

echo "Backend  : http://localhost:7001/api/health"
echo "Frontend : http://localhost:7777"
wait
