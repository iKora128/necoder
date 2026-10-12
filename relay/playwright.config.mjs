import { defineConfig, devices } from '@playwright/test';
export default defineConfig({
  testDir: './test/browser', fullyParallel: false, workers: 1, timeout: 45000,
  use: { baseURL: 'http://localhost:8791', ...devices['iPhone 13'], locale: 'ja-JP', trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }, { name: 'webkit', use: { browserName: 'webkit' } }],
  webServer: [
    // 試験用の設定（`--mode test`・cloudflare.config.ts）でビルドした物を :8791 で配る。dev ではなく build + preview に
    // するのは、`public/_headers`（CSP などの配信ヘッダー）が本番と同じに当たるため（vite の dev は当てない）。
    { command: 'npx cf build --mode test && npx vite preview --mode test', url: 'http://localhost:8791/api/health', timeout: 120000 },
    { command: 'node test/fixture.mjs', url: 'http://127.0.0.1:8792/health', timeout: 20000 },
  ],
});
