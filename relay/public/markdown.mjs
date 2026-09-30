// エージェント本文の Markdown を DOM に組む（**innerHTML を使わない**＝本文は常に textContent）。
// 対応は会話で実際に出るものだけ: 段落 / 見出し / 箇条書き / 番号付き / 引用 / 区切り線 / コードブロック /
// GFM 表 / インライン（`code`・**太字**・[リンク](https://…)・裸の URL）。
// 本文は 4000 文字で切られて届く（GUI 側の上限）ので、閉じていないフェンスや表も崩れずに出す。
// 単独 `*` の斜体は拾わない（`2 * 3` の誤爆を避ける。後読みを使うと iOS 16.3 以前で文法エラーになる）。

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function safeLink(href) {
  try {
    const url = new URL(href);
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
  } catch { return null; }
}

const INLINE = /(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)|\*\*([^*\n]+?)\*\*|\[([^\]\n]+)\]\(([^)\s]+)\)|(https?:\/\/[^\s<>()「」、。]+[^\s<>()「」、。.,:;!?'"])/g;

export function inline(text) {
  const nodes = [];
  let last = 0;
  for (const match of text.matchAll(INLINE)) {
    if (match.index > last) nodes.push(document.createTextNode(text.slice(last, match.index)));
    const [whole, , code, bold, label, href, bare] = match;
    if (code !== undefined) nodes.push(element('code', undefined, code));
    else if (bold !== undefined) { const strong = element('strong'); strong.append(...inline(bold)); nodes.push(strong); }
    else if (label !== undefined) {
      const url = safeLink(href);
      if (url) { const anchor = element('a', undefined, label); anchor.href = url; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer'; nodes.push(anchor); }
      else nodes.push(document.createTextNode(whole));
    } else if (bare !== undefined) {
      const url = safeLink(bare);
      if (url) { const anchor = element('a', undefined, bare); anchor.href = url; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer'; nodes.push(anchor); }
      else nodes.push(document.createTextNode(whole));
    }
    last = match.index + whole.length;
  }
  if (last < text.length) nodes.push(document.createTextNode(text.slice(last)));
  return nodes;
}

/// 改行はそのまま改行として見せる（チャットの流儀。日本語の文を空白で繋がない）。
function lines(target, content) {
  content.forEach((line, index) => {
    if (index) target.append(element('br'));
    target.append(...inline(line));
  });
  return target;
}

const FENCE = /^\s{0,3}(`{3,}|~{3,})\s*([\w+-]*)/;
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const RULE = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/;
const ITEM = /^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

function cells(row) {
  return row.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map(cell => cell.trim());
}

export function markdown(text) {
  const root = document.createDocumentFragment();
  const source = text.replace(/\r\n?/g, '\n').split('\n');
  let index = 0;
  let paragraph = [];
  const flush = () => {
    if (paragraph.length) root.append(lines(element('p'), paragraph));
    paragraph = [];
  };
  while (index < source.length) {
    const line = source[index];
    const fence = line.match(FENCE);
    if (fence) {
      flush();
      const body = [];
      index++;
      while (index < source.length && !source[index].trim().startsWith(fence[1])) body.push(source[index++]);
      index++; // 閉じフェンス（無ければ末尾まで）
      const pre = element('pre', 'code');
      if (fence[2]) pre.dataset.lang = fence[2];
      pre.append(element('code', undefined, body.join('\n')));
      root.append(pre);
      continue;
    }
    if (!line.trim()) { flush(); index++; continue; }
    const heading = line.match(HEADING);
    if (heading) {
      flush();
      root.append(lines(element(`h${Math.min(6, heading[1].length + 2)}`), [heading[2]]));
      index++;
      continue;
    }
    if (RULE.test(line) && !paragraph.length) { root.append(element('hr')); index++; continue; }
    if (line.includes('|') && TABLE_RULE.test(source[index + 1] ?? '')) {
      flush();
      const wrap = element('div', 'table');
      const table = element('table');
      const head = element('tr');
      for (const cell of cells(line)) head.append(lines(element('th'), [cell]));
      table.append(head);
      index += 2;
      while (index < source.length && source[index].includes('|') && source[index].trim()) {
        const row = element('tr');
        for (const cell of cells(source[index])) row.append(lines(element('td'), [cell]));
        table.append(row);
        index++;
      }
      wrap.append(table);
      root.append(wrap);
      continue;
    }
    if (QUOTE.test(line)) {
      flush();
      const quoted = [];
      while (index < source.length && QUOTE.test(source[index])) quoted.push(source[index++].match(QUOTE)[1]);
      const blockquote = element('blockquote');
      blockquote.append(markdown(quoted.join('\n')));
      root.append(blockquote);
      continue;
    }
    const item = line.match(ITEM);
    if (item) {
      flush();
      const ordered = /\d/.test(item[2]);
      const list = element(ordered ? 'ol' : 'ul');
      if (ordered) list.start = parseInt(item[2], 10);
      while (index < source.length) {
        const current = source[index].match(ITEM);
        if (!current) {
          // 箇条の続き（字下げされた行）は同じ項目へ。空行か字下げの無い行で終わる。
          const last = list.lastElementChild;
          if (last && /^\s{2,}\S/.test(source[index])) { last.append(element('br'), ...inline(source[index].trim())); index++; continue; }
          break;
        }
        if (/\d/.test(current[2]) !== ordered && current[1].length === 0) break;
        const entry = element('li');
        const depth = Math.min(3, Math.floor(current[1].replace(/\t/g, '    ').length / 2));
        if (depth) entry.dataset.depth = String(depth);
        entry.append(...inline(current[3]));
        list.append(entry);
        index++;
      }
      root.append(list);
      continue;
    }
    paragraph.push(line);
    index++;
  }
  flush();
  return root;
}

/// 一覧の 1〜2 行プレビュー用に記号を落とした平文へ。
export function plain(text) {
  return text
    .replace(/```[\s\S]*?(```|$)/g, ' ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/^\s{0,3}(#{1,6}|>|[-*+]|\d+[.)])\s+/gm, '')
    .replace(/\s+/g, ' ')
    .trim();
}
