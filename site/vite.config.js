import { defineConfig } from 'vite';
import { cloudflare } from '@cloudflare/vite-plugin';

// `cf dev` / `cf deploy` はこの設定で vite を動かす（Worker の設定は cloudflare.config.ts）。
// 配る中身はリポジトリ直下の `lp/`（README の画像やアプリのアイコンの元も兼ねるので、場所は動かさない）。
export default defineConfig({
  plugins: [cloudflare()],
  publicDir: '../lp',
});
