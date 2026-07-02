import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    port: 7777,
    proxy: {
      '/api': {
        target: 'http://localhost:7001',
        changeOrigin: true,
        // 백엔드가 아직 안 떴거나(시작 순서 경쟁) 재시작 중일 때 프록시가 raw
        // AggregateError [ECONNREFUSED] 스택을 그대로 던지지 않고, 조용히
        // 502를 돌려주고 한 줄 로그만 남긴다. 요청 자체는 여전히 실패하지만
        // (백엔드가 없으니 당연) 콘솔이 지저분해지지 않는다.
        configure: (proxy) => {
          proxy.on('error', (err, _req, res) => {
            console.warn(`[proxy] backend(7001) 연결 실패 — 백엔드가 실행 중인지 확인하세요: ${err.message}`);
            if ('writeHead' in res && !res.headersSent) {
              res.writeHead(502, { 'Content-Type': 'text/plain' });
              res.end('Backend unavailable (http://localhost:7001)');
            }
          });
        }
      }
    }
  }
});
