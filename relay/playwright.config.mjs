import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: './test/browser', fullyParallel: false, workers: 1, timeout: 45000,
  use: { baseURL: 'http://localhost:8791', ...devices['iPhone 13'], locale: 'ja-JP', trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }, { name: 'webkit', use: { browserName: 'webkit' } }],
  webServer: [
    { command: 'npx wrangler dev --config wrangler.test.jsonc --port 8791 --ip 127.0.0.1', url: 'http://localhost:8791/api/health', timeout: 60000 },
    { command: 'node test/fixture.mjs', url: 'http://127.0.0.1:8792/health', timeout: 20000 },
  ],
});
