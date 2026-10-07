import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  resolve: { conditions: ['browser'] },
  test: {
    include: ['tests/**/*.test.ts'],
    environment: 'node',
    maxWorkers: 1,
    fileParallelism: false,
    restoreMocks: true,
  },
});
