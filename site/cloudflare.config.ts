import { defineConfig } from "cf/config";

// necoder.com の LP を Cloudflare Workers（静的アセット）で配信する。`cf deploy` で出す（wrangler は使わない）。
//
// - 中身はリポジトリ直下の `lp/` をそのまま配る（vite.config.js の publicDir）。`lp/_headers` が効く。
//   Workers の静的アセットは `_redirects` に絶対 URL を書けないので、www → apex の 301 は張らず、www も同じ内容を
//   返す（正規化は index.html の <link rel="canonical"> で apex 側に寄せている）。
// - Worker（worker.js）が走るのはトップだけで、Accept-Language と cookie で ja / en を振り分ける。
// - 独自ドメインなので DNS レコードは Cloudflare 側が作る（zone は同じアカウント）。
export default defineConfig({
  worker: {
    name: "necoder",
    compatibilityDate: "2026-05-14",
    entrypoint: "./worker.js",
    workersDev: false,
    previewUrls: false,
    domains: ["necoder.com", "www.necoder.com"],
    assets: {
      htmlHandling: "auto-trailing-slash",
      notFoundHandling: "none",
      runWorkerFirst: ["/"],
    },
    observability: { enabled: false },
    env: { ASSETS: { type: "assets" } },
  },
});
