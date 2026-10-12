import { defineConfig } from "cf/config";

// リレー（control.necoder.com）の Worker。`cf deploy` で出す（wrangler は使わない）。
// `--mode test` は試験と撮影用（:8791・ローカルだけ）: 名前・レート制限・APP_ORIGIN を差し替え、独自ドメインを外す。
//
// - Durable Object（ControlRoom）は `exports` で宣言する。2026-10-12 に wrangler の `migrations`（tag v1・
//   new_sqlite_classes）から移した。storage は作った時と同じ sqlite（変えると storage_type_mismatch で断られる）。
//   exports で出した後は migrations に戻せない（一方通行）。
// - 部屋作成は誰でも叩ける（アカウント不要）ので、IP あたりの回数だけ絞る（ROOM_LIMITER）。ペアリングは一生に
//   数回なので 5 回/60s で実用上あたらない。上限は colo ごとの近似で、厳密な上限ではない（一枚目。二枚目は PoW）。
// - POW_DIFFICULTY は部屋作成に要求する PoW の先頭ゼロ bit 数。0 = 無効（既定）。レート制限で足りなくなったら
//   20 前後へ上げるだけでホストが自動で追従する。
// - 自分で運用するなら domains と APP_ORIGIN を自分の origin に替える（relay/README.md）。
export default defineConfig(({ mode }) => {
  const test = mode === "test";
  const name = test ? "necoder-control-test" : "necoder-control";
  return {
    worker: {
      name,
      compatibilityDate: "2026-03-17",
      entrypoint: "./src/worker.ts",
      workersDev: false,
      previewUrls: false,
      domains: test ? [] : ["control.necoder.com"],
      // Worker が走るのは /api/* だけ。ほかは public/ の静的配信（`public/_headers` が効く）。
      assets: { runWorkerFirst: ["/api/*"] },
      limits: { cpuMs: 10 },
      observability: { enabled: false },
      env: {
        ASSETS: { type: "assets" },
        ROOMS: { type: "durable-object", worker: name, exportName: "ControlRoom" },
        ROOM_LIMITER: { type: "rate-limit", namespace: "1001", simple: { limit: test ? 100 : 5, period: 60 } },
        APP_ORIGIN: { type: "text", value: test ? "http://localhost:8791" : "https://control.necoder.com" },
        POW_DIFFICULTY: { type: "text", value: "0" },
      },
      exports: { ControlRoom: { type: "durable-object", storage: "sqlite" } },
    },
  };
});
