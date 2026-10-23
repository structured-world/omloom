import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './web/browser',
  timeout: 30_000,
  use: { browserName: 'chromium', headless: true },
  workers: 1,
  reporter: 'list',
});
