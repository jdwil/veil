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
          '/api/agent': {
            target: 'http://127.0.0.1:3001',
            ws: true,
          },
          '/api': {
            target: 'http://127.0.0.1:3000',
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
