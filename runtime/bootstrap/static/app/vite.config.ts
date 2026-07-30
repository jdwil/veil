    import { sveltekit } from '@sveltejs/kit/vite';
    import { defineConfig } from 'vite';
    // server.watch.ignored: dual-loop gen rewrites these; watching them
    // restarts Vite mid-HMR and can crash the process (Node HMR race).
    export default defineConfig({
      plugins: [sveltekit()],
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