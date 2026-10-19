// LP（lp/index.html）の文言の後追い修正と、節の並べ替え。
//
// lp/ は Claude Design のキャンバスの書き出しから scripts/lp-build.mjs が作る。キャンバスを書き出し直すと
// 手で直した文言や並びが元に戻るので、直しはここに置いて build のたびに書き出しの原稿へ当て直す。
// 並べ替え（arrangeSections）は何度当てても変わらない。文言の直しの多くも適用済みの文言に再マッチしないが、
// MedLocal の段落を足す直しは置き換え後に置き換え前を含むので、当てた後の HTML に当て直すと段落が重なる。

// ── キャンバス側に残っている旧名の後追い修正 ────────────────────────────────
// キャンバスを再書き出しすると復活するので、ここで毎回当て直す。
// 恒久的にはキャンバス側の原稿を直すこと。
export const COPY_PATCHES = [
  ['https://github.com/iKora128/shirushi', 'https://github.com/iKora128/necoder'],
  ['Shirushi.dmg', 'necoder.dmg'],
  // Windows 対応（W0〜W6 完了・2026-08-25）を配布文言に反映。
  // 2つ目は `</span>` まで含めて検索し、適用済みテキストに再マッチしないようにしている。
  ['necoder.dmg をダウンロード', 'Mac / Windows 版をダウンロード'],
  ['無料 · macOS 13+（Apple Silicon）</span>', '無料 · macOS 13+（Apple Silicon）/ Windows 10+（x64）</span>'],
  // MedLocal（GENSHI AI の主力プロダクト）を作者セクションに載せる。キャンバス側の原稿には無い。
  // 2 つ目は <b> を外して medlocal.jp へのリンクにする。どちらも適用済みテキストには再マッチしない。
  ['医療の現場へ届けるところまでを自分の手で通します。</p>', '医療の現場へ届けるところまでを自分の手で通します。</p>\n          <p style="font-size: 16.5px; line-height: 1.9; margin: 20px 0 0; max-width: 56ch">主力プロダクトは <a href="https://medlocal.jp" target="_blank" rel="noopener" style="font-weight: 700; text-decoration: underline; text-underline-offset: 3px; text-decoration-thickness: 1px">MedLocal</a> — 院内に置く医療 LLM です。退院サマリーの下書き、外来の音声カルテ、紹介状の OCR を、患者データを院外に出さずに院内のマシンだけで動かします。2026 年 9 月、<a href="https://www.itmedia.co.jp/aiplus/article/2609/17/2000001587/" target="_blank" rel="noopener" style="text-decoration: underline; text-underline-offset: 3px; text-decoration-thickness: 1px">ITmedia の記事</a> に GENSHI AI 代表として取材を受けました。</p>'],
  ['<div><b>MedLocal</b><span>', '<div><a href="https://medlocal.jp" target="_blank" rel="noopener" style="font-weight: 700; text-decoration: none">MedLocal</a><span>'],
  // 冒頭は「Rust 製で速くて軽い」を先に出す（2026-10-18・本人「色よりも Rust のハイパフォーマンスを最初に」）。
  // README・リポジトリの説明・アプリの About と同じ言い方。GPUI は知らない人に伝わらないので表の文言から外す。
  ['AIエージェント時代の、<br>次世代コードエディタ。', 'Rust で書かれた、<br>速くて軽いコードエディタ。'],
  ['Electron とはおさらば。Rust + GPUI で低メモリ · 60fps', 'Electron とはおさらば。Rust 製で低メモリ · 60fps'],
  ['Rust で書かれ、GPUI が GPU で直接描画します。', 'Rust で書かれ、GPU で直接描画します。'],
  ['GPUI ネイティブ UI', 'ネイティブ UI'],
];

// ヒーローの要点と本文の節の並び。性能を先頭に置く（上と同じ 2026-10-18 の判断）。
const HERO_POINT_ORDER = ['ハイパフォーマンス', '手持ちの AI エージェント', '色分け UI', 'マルチプレクサ'];
const SECTION_ORDER = ['perf', 'ai', 'iro', 'fleet', 'editor'];
// 先頭の節だけ上の余白が小さい（ヒーローの下の区切り線との間）。並べ替えた後も先頭に付け直す。
const FIRST_SECTION_PADDING = 'padding: 96px 0 0';
const OTHER_SECTION_PADDING = 'padding: 110px 0 0';

/** 連続して並ぶ `items` を `order` の順に並べ直す（間の空白は最初の 2 つの間のものを使う）。 */
function reorderRun(html, items, order) {
  if (items.length !== order.length) {
    throw new Error(`並べ替えの対象が ${items.length} 個（想定 ${order.length} 個）`);
  }
  const first = items[0];
  const last = items[items.length - 1];
  const separator = html.slice(first.index + first.text.length, items[1].index);
  const sorted = order.map((key) => {
    const item = items.find((candidate) => candidate.key === key);
    if (!item) throw new Error(`並べ替えの対象が見つからない: ${key}`);
    return item.text;
  });
  return html.slice(0, first.index) + sorted.join(separator) + html.slice(last.index + last.text.length);
}

/** ヒーローの要点・ナビ・本文の節を、性能が先頭の並びにそろえ、節の番号と先頭の余白を付け直す。 */
export function arrangeSections(html) {
  const points = [...html.matchAll(/<div[^>]*><span[^>]*><\/span><p[^>]*><b>([^<]+)<\/b>[^\n]*<\/p><\/div>/g)]
    .filter((match) => HERO_POINT_ORDER.includes(match[1]))
    .map((match) => ({ key: match[1], text: match[0], index: match.index }));
  html = reorderRun(html, points, HERO_POINT_ORDER);

  const navLinks = [...html.matchAll(/<a href="#(perf|ai|iro|fleet|editor)"[^>]*>[^<]*<\/a>/g)].map((match) => ({
    key: match[1],
    text: match[0],
    index: match.index,
  }));
  html = reorderRun(html, navLinks, SECTION_ORDER);

  const sections = [...html.matchAll(/<section id="(perf|ai|iro|fleet|editor)"[\s\S]*?<\/section>/g)].map((match) => ({
    key: match[1],
    text: match[0],
    index: match.index,
  }));
  html = reorderRun(html, sections, SECTION_ORDER);

  SECTION_ORDER.forEach((id, position) => {
    const opening = new RegExp(`<section id="${id}" style="padding: \\d+px 0 0">`);
    const padding = position === 0 ? FIRST_SECTION_PADDING : OTHER_SECTION_PADDING;
    html = html.replace(opening, `<section id="${id}" style="${padding}">`);
    const start = html.indexOf(`<section id="${id}"`);
    const number = String(position + 1).padStart(2, '0');
    const numbered = html.slice(start).replace(/>\d\d<\/span>/, `>${number}</span>`);
    html = html.slice(0, start) + numbered;
  });
  return html;
}

// ── ダウンロード: OS を見分けてボタンを 1 つだけ大きく出す（2026-10-19・本人が 5 案から選んだ案 2）──
// 以前は「Mac / Windows 版をダウンロード」が GitHub の Releases の一覧を開くだけで、普通の人は GitHub の
// ページで迷った。最新の正式版のファイルを直接落とす（releases/latest/download/<名前>）。Mac で開けば
// Mac 用・Windows なら Windows 用だけを大きく出し、もう片方は下の小さなリンク。見分けられない時・
// スクリプトが止まっている時は両方出す。?os=windows / ?os=mac で確かめられる。
// 「版」は使わない（本人「版という日本語は使いません、バージョンです」）。
// 書き出しの markup（ナビ・ヒーロー・下の帯のボタン）を置き換える。当てた後の HTML に当て直しても変わらない。
const RELEASES = 'https://github.com/iKora128/necoder/releases';
const MAC_DOWNLOAD = `${RELEASES}/latest/download/necoder.dmg`;
const WINDOWS_DOWNLOAD = `${RELEASES}/latest/download/necoder-windows-x64-setup.exe`;
const OS_SCRIPT = `<script>
(function () {
  var forced = new URLSearchParams(location.search).get('os');
  var platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || '';
  var agent = navigator.userAgent || '';
  var os = forced === 'windows' || forced === 'mac' ? forced
    : /Win/i.test(platform) || /Windows/.test(agent) ? 'windows'
    : /Mac/i.test(platform) || /Macintosh|Mac OS X/.test(agent) ? 'mac' : '';
  if (os) document.documentElement.dataset.os = os;
})();
</script>
<style>
html[data-os=mac] .dl-win, html[data-os=windows] .dl-mac { display: none !important; }
html:not([data-os]) .dl-other { display: none !important; }
</style>
`;
const SMALL_NOTE = 'font-size: 12.5px; color: color-mix(in srgb, var(--color-text) 65%, transparent)';
const BIG_BUTTON = 'white-space: nowrap; text-decoration: none; padding: 14px 28px; font-size: 16px';
const BAND_BUTTON =
  'white-space: nowrap; text-decoration: none; color: var(--color-bg); border: 1px solid var(--color-bg); padding: 14px 28px; font-size: 16px';
const MAC_NOTE = '無料 · macOS 13 以降（Apple Silicon）';
const WINDOWS_NOTE = '無料 · Windows 10 以降（x64）';

export function applyDownloads(html) {
  if (!html.includes('html[data-os=mac]')) html = html.replace('</head>', `${OS_SCRIPT}</head>`);

  html = html.replace(
    /<a class="btn btn-primary" href="https:\/\/github\.com\/iKora128\/necoder\/releases" style="white-space: nowrap; text-decoration: none">([^<]+)<\/a>/,
    (_, label) =>
      `<a class="btn btn-primary dl-mac" href="${MAC_DOWNLOAD}" style="white-space: nowrap; text-decoration: none">${label}</a>` +
      `<a class="btn btn-primary dl-win" href="${WINDOWS_DOWNLOAD}" style="white-space: nowrap; text-decoration: none">${label}</a>`,
  );

  html = html.replace(
    /        <div style="display: flex; gap: 12px; flex-wrap: wrap; padding-bottom: 4px">\n[\s\S]*?GitHub<\/a>\n        <\/div>/,
    `        <div style="display: grid; gap: 10px; justify-items: start; padding-bottom: 4px">
          <div style="display: flex; gap: 12px; flex-wrap: wrap">
            <a class="btn btn-primary dl-mac" href="${MAC_DOWNLOAD}" style="${BIG_BUTTON}">Mac 用ダウンロード</a>
            <a class="btn btn-primary dl-win" href="${WINDOWS_DOWNLOAD}" style="${BIG_BUTTON}">Windows 用ダウンロード</a>
            <a class="btn btn-secondary" href="https://github.com/iKora128/necoder" style="${BIG_BUTTON}">GitHub</a>
          </div>
          <span class="dl-mac" style="${SMALL_NOTE}">${MAC_NOTE}<span class="dl-other"> · <a href="${WINDOWS_DOWNLOAD}" style="color: inherit">Windows 用はこちら</a></span></span>
          <span class="dl-win" style="${SMALL_NOTE}">${WINDOWS_NOTE}<span class="dl-other"> · <a href="${MAC_DOWNLOAD}" style="color: inherit">Mac 用はこちら</a></span></span>
        </div>`,
  );

  html = html.replace(
    /      <div style="display: flex; gap: 12px; flex-wrap: wrap; margin-top: 36px; align-items: center">\n[\s\S]*?<\/div>\n/,
    `      <div style="display: grid; gap: 12px; justify-items: start; margin-top: 36px">
        <a class="btn dl-mac" href="${MAC_DOWNLOAD}" style="${BAND_BUTTON}">Mac 用ダウンロード</a>
        <a class="btn dl-win" href="${WINDOWS_DOWNLOAD}" style="${BAND_BUTTON}">Windows 用ダウンロード</a>
        <span class="dl-mac" style="font-size: 13px; opacity: 0.85">${MAC_NOTE}<span class="dl-other"> · <a href="${WINDOWS_DOWNLOAD}" style="color: var(--color-bg)">Windows 用はこちら</a></span> · <a href="${RELEASES}" style="color: var(--color-bg)">ほかのバージョン</a></span>
        <span class="dl-win" style="font-size: 13px; opacity: 0.85">${WINDOWS_NOTE}<span class="dl-other"> · <a href="${MAC_DOWNLOAD}" style="color: var(--color-bg)">Mac 用はこちら</a></span> · <a href="${RELEASES}" style="color: var(--color-bg)">ほかのバージョン</a></span>
      </div>
`,
  );
  return html;
}
