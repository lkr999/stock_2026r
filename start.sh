#!/usr/bin/env bash
# 백앤드(7001)·프론트앤드(7777) 종료 후 재시작 (Rust + SvelteKit)
ROOT="$(cd "$(dirname "$0")" && pwd)"
LOG_DIR="$ROOT/.logs"
mkdir -p "$LOG_DIR"

# 포트를 점유한 프로세스를 종료하고, 포트가 풀릴 때까지 대기
kill_port() {
  local port="$1" pids i=0
  pids=$(lsof -ti tcp:"$port" -s tcp:LISTEN 2>/dev/null || true)
  if [ -n "$pids" ]; then
    kill -9 $pids 2>/dev/null || true
    while [ $i -lt 5 ]; do
      sleep 1
      [ -z "$(lsof -ti tcp:"$port" -s tcp:LISTEN 2>/dev/null)" ] && return 0
      i=$((i + 1))
    done
    echo "경고: 포트 $port 가 5초 후에도 해제되지 않음" >&2
  fi
}

echo "=== 종료 ==="
kill_port 7001
kill_port 7777

echo "=== 백앤드 빌드 & 시작 (Rust) ==="
cd "$ROOT/backend"
cargo build --release   # 첫 실행은 의존성 컴파일로 시간이 걸립니다
nohup ./target/release/caginalp-trader > "$LOG_DIR/backend.log" 2>&1 &
BACKEND_PID=$!
echo "PID $BACKEND_PID | 로그: $LOG_DIR/backend.log"

echo "=== 프론트앤드 시작 (SvelteKit) ==="
cd "$ROOT/frontend"
[ -d node_modules ] || npm install
nohup npm run dev -- --port 7777 > "$LOG_DIR/frontend.log" 2>&1 &
FRONTEND_PID=$!
echo "PID $FRONTEND_PID | 로그: $LOG_DIR/frontend.log"

# 백앤드가 실제로 응답할 때까지 대기 (최대 30초)
echo -n "=== health 대기 "
OK=0
for i in $(seq 1 30); do
  sleep 1
  if ! kill -0 "$BACKEND_PID" 2>/dev/null; then
    echo ""; echo "오류: 백앤드 프로세스가 종료됨. 로그 확인:" >&2
    tail -20 "$LOG_DIR/backend.log" >&2; exit 1
  fi
  if curl -fsS http://localhost:7001/api/health >/dev/null 2>&1; then
    OK=1; echo " OK ($i 초)"; break
  fi
  echo -n "."
done
if [ "$OK" -ne 1 ]; then
  echo ""; echo "오류: 백앤드가 30초 내에 응답하지 않음. 로그 확인:" >&2
  tail -20 "$LOG_DIR/backend.log" >&2; exit 1
fi

echo ""
echo "Backend  : http://localhost:7001/api/health"
echo "Frontend : http://localhost:7777"
echo ""
echo "로그 확인:"
echo "  tail -f $LOG_DIR/backend.log"
echo "  tail -f $LOG_DIR/frontend.log"
