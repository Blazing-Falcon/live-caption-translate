import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

/** Test-only dev server for the Playwright harness pages under e2e/harness. Never used by `npm run build`. */
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { host: '127.0.0.1', port: 1431, strictPort: true },
});
