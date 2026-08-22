#!/usr/bin/env node
// Claude Design キャンバスの「バンドル HTML」を、普通の静的サイト（lp/）に展開する。
//
// キャンバスからの書き出しは 1 枚の巨大 HTML（画像・フォント・ランタイムを base64 で内包し、
// 起動時に JS で解凍して blob URL に差し替える）。そのまま配ると 14MB を DL してから
// 描画が始まるので、リソースを実ファイルに戻して普通に配信できる形に変換する。
//
// 使い方:
//   node scripts/lp-build.mjs "~/Downloads/necoder LP.html"
//
// 出力: lp/index.html + lp/assets/**（既存アセットとハッシュ一致したものは再利用する）

import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const lpRoot = path.join(repoRoot, 'lp');

// ── キャンバス側に残っている旧名の後追い修正 ────────────────────────────────
// キャンバスを再書き出しすると復活するので、ここで毎回当て直す。
// 恒久的にはキャンバス側の原稿を直すこと。
const COPY_PATCHES = [
  ['https://github.com/iKora128/shirushi', 'https://github.com/iKora128/necoder'],
  ['Shirushi.dmg', 'necoder.dmg'],
];

// Archivo はラテン専用なので、日本語のフォールバックを足す。
const FONT_STACK_PATCHES = [
  [
    '--font-body: "Archivo", system-ui, sans-serif;',
    '--font-body: "Archivo", system-ui, "Hiragino Sans", "Hiragino Kaku Gothic ProN", "Yu Gothic UI", "Noto Sans JP", sans-serif;',
  ],
  [
    '--font-heading: "Archivo", system-ui, sans-serif;',
    '--font-heading: "Archivo", system-ui, "Hiragino Sans", "Hiragino Kaku Gothic ProN", "Yu Gothic UI", "Noto Sans JP", sans-serif;',
  ],
];

const SITE_URL = 'https://necoder.com';
const HEAD_META = `<title>necoder — AI エージェント時代の、次世代コードエディタ</title>
<meta name="description" content="色分け UI で、どの AI が走りどれに指示しているかが一目でわかる。ACP 対応のエージェントが契約中のサブスクのまま動く、Rust + GPUI 製のネイティブコードエディタ。">
<link rel="canonical" href="${SITE_URL}/">
<link rel="icon" href="assets/img/favicon.png">
<meta property="og:type" content="website">
<meta property="og:url" content="${SITE_URL}/">
<meta property="og:site_name" content="necoder">
<meta property="og:title" content="necoder — AI エージェント時代の、次世代コードエディタ">
<meta property="og:description" content="色分け UI・ACP 対応エージェント・Rust + GPUI のネイティブ性能。AI と並走するためのコードエディタ。">
<meta property="og:image" content="${SITE_URL}/assets/img/hero.png">
<meta property="og:locale" content="ja_JP">
<meta name="twitter:card" content="summary_large_image">
<meta name="theme-color" content="#faf7f2">
<link rel="stylesheet" href="assets/css/responsive.css">`;

const MIME_EXT = {
  'image/svg+xml': 'svg', 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif',
  'image/webp': 'webp', 'video/mp4': 'mp4', 'text/javascript': 'js',
  'application/javascript': 'js', 'text/css': 'css', 'font/woff2': 'woff2',
};

function readSection(src, type) {
  const open = `<script type="__bundler/${type}">`;
  const start = src.indexOf(open);
  if (start < 0) throw new Error(`バンドルに __bundler/${type} セクションがない`);
  const from = start + open.length;
  return src.slice(from, src.indexOf('</script>', from)).trim();
}

function sha256(buffer) {
  return crypto.createHash('sha256').update(buffer).digest('hex');
}

// lp/assets 配下の既存ファイルをハッシュで引けるようにする（キャンバスは同じ画像を内包している）。
function indexExistingAssets(dir) {
  const byHash = new Map();
  const walk = (current) => {
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else byHash.set(sha256(fs.readFileSync(full)), path.relative(lpRoot, full));
    }
  };
  if (fs.existsSync(dir)) walk(dir);
  return byHash;
}

function slugify(text) {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
}

const bundlePath = process.argv[2];
if (!bundlePath) {
  console.error('使い方: node scripts/lp-build.mjs <キャンバス書き出しの HTML>');
  process.exit(1);
}
const source = fs.readFileSync(bundlePath.replace(/^~/, process.env.HOME ?? '~'), 'utf8');

const manifest = JSON.parse(readSection(source, 'manifest'));
const externals = JSON.parse(readSection(source, 'ext_resources'));
let template = JSON.parse(readSection(source, 'template'));

// ── UUID → 出力パスを決める ────────────────────────────────────────────────
const externalByUuid = new Map(externals.map((e) => [e.uuid, e.id]));
const runtimeUuid = template.match(/<script src="([0-9a-f-]{36})"><\/script>/)?.[1];

// フォントは @font-face 直前の /* subset */ コメントから名前を作る。
const fontSubsets = new Map();
for (const match of template.matchAll(/\/\*\s*([a-z0-9-]+)\s*\*\/\s*@font-face\s*\{(.*?)\}/gs)) {
  const family = match[2].match(/font-family:\s*'([^']+)'/)?.[1];
  const uuid = match[2].match(/url\("([0-9a-f-]{36})"\)/)?.[1];
  if (family && uuid) fontSubsets.set(uuid, `${slugify(family)}-${match[1]}`);
}

const existing = indexExistingAssets(path.join(lpRoot, 'assets'));
const writes = [];
const rewrites = new Map();
const resourceMap = {}; // window.__resources: CDN URL → 自前配信パス

for (const [uuid, entry] of Object.entries(manifest)) {
  let bytes = Buffer.from(entry.data, 'base64');
  if (entry.compressed) bytes = zlib.gunzipSync(bytes);
  const ext = MIME_EXT[entry.mime] ?? 'bin';

  let target = existing.get(sha256(bytes)); // 既存アセットと同一ならそれを使う
  if (!target) {
    if (externalByUuid.has(uuid)) {
      target = `assets/js/${path.basename(new URL(externalByUuid.get(uuid)).pathname)}`;
    } else if (uuid === runtimeUuid) {
      target = 'assets/js/dc-runtime.js';
    } else if (fontSubsets.has(uuid)) {
      target = `assets/font/${fontSubsets.get(uuid)}.woff2`;
    } else if (ext === 'svg') {
      const title = bytes.toString('utf8').match(/<title>([^<]+)<\/title>/)?.[1];
      target = `assets/icon/${title ? slugify(title) : uuid.slice(0, 8)}.svg`;
    } else if (ext === 'js') {
      target = `assets/js/ds-bundle-${uuid.slice(0, 8)}.js`;
    } else {
      target = `assets/img/${uuid.slice(0, 8)}.${ext}`;
    }
    writes.push([target, bytes]);
  }

  rewrites.set(uuid, target);
  if (externalByUuid.has(uuid)) resourceMap[externalByUuid.get(uuid)] = target;
}

for (const [target, bytes] of writes) {
  const full = path.join(lpRoot, target);
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, bytes);
}

// ── テンプレートを書き換える ──────────────────────────────────────────────
for (const [uuid, target] of rewrites) template = template.replaceAll(uuid, target);

for (const [before, after] of [...COPY_PATCHES, ...FONT_STACK_PATCHES]) {
  const hits = template.split(before).length - 1;
  if (hits > 0) console.log(`  patch ×${hits}: ${before} → ${after}`);
  template = template.replaceAll(before, after);
}

template = template.replace('<html>', '<html lang="ja">');

// dc-runtime は React を unpkg から読む。__resources に自前パスを入れると
// そちらを使うので、外部 CDN 依存なしで動く。
const resourceShim = `<script>window.__resources=${JSON.stringify(resourceMap)};</script>`;
template = template.replace(
  '<script src="assets/js/dc-runtime.js"></script>',
  `${HEAD_META}\n${resourceShim}\n<script src="assets/js/dc-runtime.js"></script>`,
);

const indexPath = path.join(lpRoot, 'index.html');
fs.writeFileSync(indexPath, template);

// sitemap は 1 ページだけ。lastmod をビルド日に保つのが目的なので毎回書き出す。
const today = new Date().toISOString().slice(0, 10);
fs.writeFileSync(
  path.join(lpRoot, 'sitemap.xml'),
  `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>${SITE_URL}/</loc>
    <lastmod>${today}</lastmod>
    <changefreq>weekly</changefreq>
  </url>
</urlset>
`,
);

const leftover = [...template.matchAll(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g)];
if (leftover.length > 0) console.warn(`  ⚠ 未解決の UUID が ${leftover.length} 件残っている`);

console.log(`\nlp/index.html (${(template.length / 1024).toFixed(1)}KB)`);
console.log(`新規アセット ${writes.length} 件 / 既存流用 ${rewrites.size - writes.length} 件`);
for (const [target, bytes] of writes) console.log(`  + ${target} (${(bytes.length / 1024).toFixed(1)}KB)`);
