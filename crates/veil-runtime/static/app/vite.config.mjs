import { defineConfig } from 'vite';

export default defineConfig({
  server: {
    port: 5180,
    proxy: {
      '/api': 'http://localhost:3000',
      '/bus': 'http://localhost:3000',
      '/health': 'http://localhost:3000',
    }
  }
});
