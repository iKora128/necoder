import { defineConfig } from 'vite';
import { cloudflare } from '@cloudflare/vite-plugin';

// `cf dev` / `cf build` / `cf deploy` はこの設定で vite を動かす（Worker の設定は cloudflare.config.ts）。
// `public/` はそのまま静的資産になる（vite の publicDir の既定）。
// `--mode test`（試験・撮影）は :8791 に固定する（playwright.config.mjs と test/preview.mjs が見る）。
const fixed = { port: 8791, host: '127.0.0.1', strictPort: true };
export default defineConfig(({ mode }) => ({
  plugins: [cloudflare()],
  server: mode === 'test' ? fixed : undefined,
  preview: mode === 'test' ? fixed : undefined,
}));
