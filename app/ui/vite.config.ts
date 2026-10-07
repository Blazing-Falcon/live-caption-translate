import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

const page = (name: string): string => fileURLToPath(new URL(`./${name}.html`, import.meta.url));

export default defineConfig({
  plugins: [svelte()],
  base: './',
  clearScreen: false,
  server: { host: '127.0.0.1', port: 1420, strictPort: true },
  build: {
    target: 'chrome111',
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: { overlay: page('overlay'), control: page('control') },
    },
  },
});
