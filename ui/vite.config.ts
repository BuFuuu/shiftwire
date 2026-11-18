import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// `npm run dev` serves the UI with hot reload and forwards API/WebSocket calls
// to the running daemon (`shiftwire start`), so UI work needs no Rust rebuilds.
export default defineConfig({
  plugins: [svelte()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:7777',
      '/ws': { target: 'ws://127.0.0.1:7777', ws: true },
    },
  },
  build: { outDir: 'dist', emptyOutDir: true },
});
