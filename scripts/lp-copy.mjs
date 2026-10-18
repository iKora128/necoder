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
