import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

// Single ProductHost (veil-runtime bootstrap) — IDE kernel + platform API + agent.
// Default: :8080 (VEIL_PORT / pure-runtime). Override: VEIL_RUNTIME_PROXY=http://127.0.0.1:3210
//
// Do NOT split /api → veil_bin and /api/agent → veil serve --multi.
// That dual-process setup is retired (see docs/ADR_SINGLE_PRODUCT_HOST.md).

const runtimeTarget =
  process.env.VEIL_RUNTIME_PROXY ||
  process.env.VEIL_API_ORIGIN ||
  'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [sveltekit()],
  optimizeDeps: {
    exclude: ['@aether-ui/core'],
  },
  ssr: {
    noExternal: ['@aether-ui/core'],
  },
  server: {
    proxy: {
      // Agent WebSocket + REST (same origin as ProductHost)
      '/api/agent': {
        target: runtimeTarget,
        changeOrigin: true,
        ws: true,
        configure: (proxy) => {
          proxy.on('error', (err) => {
            console.error('[vite proxy /api/agent]', err.message);
          });
        },
      },
      '/api': {
        target: runtimeTarget,
        changeOrigin: true,
        ws: true,
      },
      // Same-origin IDE SPA when ProductHost serves /viewer
      '/viewer': {
        target: runtimeTarget,
        changeOrigin: true,
      },
    },
    watch: {
      ignored: [
        '**/svelte.config.js',
        '**/tsconfig.json',
        '**/package.json',
        '**/package-lock.json',
      ],
    },
  },
});
