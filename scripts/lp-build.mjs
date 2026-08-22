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

// テーマ: 既定は OS 追従、ナビのトグルで明示指定したらそれを localStorage に憶える。
// <head> で流すのは、描画前に data-theme を当てて明→暗のチラつきを消すため。
// クリックは document への委譲にしてある（ナビは dc-runtime が React で描き直すので、
// 要素に直接ハンドラを付けると再描画で外れる）。
const THEME_SCRIPT = `<script>
(function () {
  var KEY = 'necoder-theme';
  var root = document.documentElement;
  function apply(theme) {
    if (theme === 'dark' || theme === 'light') root.setAttribute('data-theme', theme);
    else root.removeAttribute('data-theme');
  }
  function stored() { try { return localStorage.getItem(KEY); } catch (error) { return null; } }
  apply(stored());
  document.addEventListener('click', function (event) {
    var target = event.target;
    var button = target && target.closest ? target.closest('.theme-toggle') : null;
    if (!button) return;
    var systemDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    var current = root.getAttribute('data-theme') || (systemDark ? 'dark' : 'light');
    var next = current === 'dark' ? 'light' : 'dark';
    if ((next === 'dark') === systemDark) {
      // OS 設定と同じに戻った → 記憶を捨てて「OS 追従」へ戻す
      try { localStorage.removeItem(KEY); } catch (error) {}
      apply(null);
      return;
    }
    try { localStorage.setItem(KEY, next); } catch (error) {}
    apply(next);
  });
})();
<\/script>`;

/// ナビに挿すテーマトグル（記号は「押すと切り替わる先」を出す。CSS 側で出し分け）。
const THEME_TOGGLE =
  '<button class="theme-toggle" type="button" aria-label="テーマを切り替え">' +
  '<span class="theme-toggle-to-dark" aria-hidden="true">\u263E</span>' +
  '<span class="theme-toggle-to-light" aria-hidden="true">\u2600</span>' +
  '</button>';

const SITE_URL = 'https://necoder.com';
const HREFLANG = `<link rel="alternate" hreflang="ja" href="${SITE_URL}/">
<link rel="alternate" hreflang="en" href="${SITE_URL}/en/">
<link rel="alternate" hreflang="x-default" href="${SITE_URL}/">`;

// 共通の <head>（言語ごとに title/description/canonical だけ差し替える）。
const headMeta = ({ title, description, ogDescription, canonical, locale }) => `<title>${title}</title>
<meta name="description" content="${description}">
<link rel="canonical" href="${canonical}">
${HREFLANG}
<link rel="icon" href="/assets/img/favicon.png">
<meta property="og:type" content="website">
<meta property="og:url" content="${canonical}">
<meta property="og:site_name" content="necoder">
<meta property="og:title" content="${title}">
<meta property="og:description" content="${ogDescription}">
<meta property="og:image" content="${SITE_URL}/assets/img/hero.png">
<meta property="og:locale" content="${locale}">
<meta name="twitter:card" content="summary_large_image">
<meta name="theme-color" content="#f3f2f2" media="(prefers-color-scheme: light)">
<meta name="theme-color" content="#171514" media="(prefers-color-scheme: dark)">
<link rel="stylesheet" href="/assets/css/theme.css">
<link rel="stylesheet" href="/assets/css/responsive.css">
${THEME_SCRIPT}`;

// 出力する 2 ページ。ja が正（canonical / x-default）、en は /en/ に置く。
const PAGES = [
  {
    lang: 'ja',
    dir: '',
    translate: null,
    switchTo: { href: '/?lang=en', hreflang: 'en', label: 'EN', title: 'Read this page in English' },
    meta: {
      title: 'necoder — AI エージェント時代の、次世代コードエディタ',
      description:
        '色分け UI で、どの AI が走りどれに指示しているかが一目でわかる。ACP 対応のエージェントが契約中のサブスクのまま動く、Rust + GPUI 製のネイティブコードエディタ。',
      ogDescription:
        '色分け UI・ACP 対応エージェント・Rust + GPUI のネイティブ性能。AI と並走するためのコードエディタ。',
      canonical: `${SITE_URL}/`,
      locale: 'ja_JP',
    },
  },
  {
    lang: 'en',
    dir: 'en',
    translate: 'lp/i18n/en.json',
    switchTo: { href: '/?lang=ja', hreflang: 'ja', label: '日本語', title: 'このページを日本語で読む' },
    meta: {
      title: 'necoder — the next-generation code editor for the AI agent era',
      description:
        'A color-coded UI shows which AI is running and which one you are talking to. ACP-compatible agents run on the subscription you already have. Native code editor built in Rust + GPUI.',
      ogDescription:
        'Color-coded UI, ACP-compatible agents, native Rust + GPUI performance. A code editor for working alongside AI.',
      canonical: `${SITE_URL}/en/`,
      locale: 'en_US',
    },
  },
];

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

// ── テンプレートを書き換える（言語共通の部分）──────────────────────────────
// アセットはルート絶対で参照する。/en/ からも同じ 1 本を指すため。
for (const [uuid, target] of rewrites) template = template.replaceAll(uuid, `/${target}`);

for (const [before, after] of [...COPY_PATCHES, ...FONT_STACK_PATCHES]) {
  const hits = template.split(before).length - 1;
  if (hits > 0) console.log(`  patch ×${hits}: ${before} → ${after}`);
  template = template.replaceAll(before, after);
}

// dc-runtime は React を unpkg から読む。__resources に自前パスを入れると
// そちらを使うので、外部 CDN 依存なしで動く。
const resourceShim = `<script>window.__resources=${JSON.stringify(
  Object.fromEntries(Object.entries(resourceMap).map(([url, target]) => [url, `/${target}`])),
)};</script>`;

// ナビのダウンロードボタンの直前に、テーマトグルと言語切替を挿す。書き出しの markup には
// 無いので足すが、素の <button> / <a> なので dc-runtime（React）はそのまま描画する。
const navDownloadButton =
  '<a class="btn btn-primary" href="https://github.com/iKora128/necoder/releases" style="white-space: nowrap; text-decoration: none">';
if (!template.includes(navDownloadButton)) {
  console.warn('  ⚠ ナビのダウンロードボタンが見つからず、トグル類を挿せなかった');
}

const jpText = /[ぁ-んァ-ヶ一-龥]/;

for (const page of PAGES) {
  let html = template;

  const languageSwitch =
    `<a class="lang-switch" href="${page.switchTo.href}" hreflang="${page.switchTo.hreflang}"` +
    ` title="${page.switchTo.title}">${page.switchTo.label}</a>`;
  html = html.replace(
    navDownloadButton,
    `${languageSwitch}\n    ${THEME_TOGGLE}\n    ${navDownloadButton}`,
  );

  html = html.replace(
    '<script src="/assets/js/dc-runtime.js"></script>',
    `${headMeta(page.meta)}\n${resourceShim}\n<script src="/assets/js/dc-runtime.js"></script>`,
  );
  html = html.replace('<html>', `<html lang="${page.lang}">`);

  if (page.translate) {
    const dictionary = JSON.parse(fs.readFileSync(path.join(repoRoot, page.translate), 'utf8'));
    // 長い順に置換する。短い語（"色"）が長い文の一部を先に食うのを避けるため。
    const entries = Object.entries(dictionary)
      .filter(([key]) => !key.startsWith('_'))
      .sort((a, b) => b[0].length - a[0].length);
    let replaced = 0;
    for (const [before, after] of entries) {
      const hits = html.split(before).length - 1;
      replaced += hits;
      if (hits > 0) html = html.replaceAll(before, after);
    }
    // 訳し漏れ（= キャンバスに文言を足したのに en.json を更新していない）を出す。
    // <script> の中（自前スクリプトの日本語コメント）と、言語切替リンク自体は対象外。
    const scanned = html
      .replace(/<script[\s\S]*?<\/script>/g, '')
      .replaceAll(languageSwitch, '');
    const untranslated = new Set();
    for (const match of scanned.matchAll(/>([^<>]+)</g)) {
      const text = match[1].trim();
      if (text && jpText.test(text)) untranslated.add(text);
    }
    for (const match of scanned.matchAll(/(alt|aria-label|title|content)="([^"]*)"/g)) {
      if (jpText.test(match[2])) untranslated.add(`[${match[1]}] ${match[2]}`);
    }
    console.log(`  en: ${replaced} 箇所を置換`);
    if (untranslated.size > 0) {
      console.warn(`  ⚠ en に日本語が ${untranslated.size} 件残っている（lp/i18n/en.json に追記して）:`);
      for (const text of untranslated) console.warn(`     ${text.slice(0, 70)}`);
    }
  }

  const outDir = path.join(lpRoot, page.dir);
  fs.mkdirSync(outDir, { recursive: true });
  fs.writeFileSync(path.join(outDir, 'index.html'), html);
  console.log(`lp/${page.dir ? `${page.dir}/` : ''}index.html (${(html.length / 1024).toFixed(1)}KB)`);

  const leftover = [...html.matchAll(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g)];
  if (leftover.length > 0) console.warn(`  ⚠ 未解決の UUID が ${leftover.length} 件残っている`);
}

// sitemap は ja / en の 2 本。lastmod をビルド日に保つのが目的なので毎回書き出す。
const today = new Date().toISOString().slice(0, 10);
const urls = PAGES.map(
  (page) => `  <url>
    <loc>${page.meta.canonical}</loc>
    <lastmod>${today}</lastmod>
    <changefreq>weekly</changefreq>
  </url>`,
).join('\n');
fs.writeFileSync(
  path.join(lpRoot, 'sitemap.xml'),
  `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${urls}
</urlset>
`,
);

console.log(`\n新規アセット ${writes.length} 件 / 既存流用 ${rewrites.size - writes.length} 件`);
for (const [target, bytes] of writes) console.log(`  + ${target} (${(bytes.length / 1024).toFixed(1)}KB)`);
