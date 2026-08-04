    import { sveltekit } from '@sveltejs/kit/vite';
    import { defineConfig } from 'vite';
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
          '/api/agent/chat': {
            target: 'http://127.0.0.1:3000',
            ws: true,
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
