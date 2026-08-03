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