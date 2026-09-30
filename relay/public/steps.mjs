// ツール実行の 1 項目を「何をしたか」の言葉に直す（読む / 検索 / 編集 / 書き込み / Web / ツール / 実行）。
// 本文は `{タイトル}\n{引数}\n{結果}`（crates/agent_panel/src/remote.rs）。ファイル系は `Read docs/X.md (1 - 60)`
// のように「動詞 + 対象」、Claude Code の Bash はタイトルがコマンドそのもので届く。DOM に触らない（テスト可能）。
export const KINDS = ['read', 'search', 'edit', 'write', 'web', 'tool', 'run'];

const FILE_TOOLS = { Read: 'read', Edit: 'edit', Editing: 'edit', MultiEdit: 'edit', NotebookEdit: 'edit', Write: 'write' };
const READERS = new Set(['cat', 'head', 'tail', 'less', 'nl', 'wc', 'sed', 'ls', 'tree', 'stat', 'jq', 'file', 'cut', 'diff', 'du']);
const SEARCHERS = new Set(['grep', 'egrep', 'rg', 'find', 'fd', 'ag', 'ack']);
const CD = /^cd\s+(?:"[^"]*"|'[^']*'|\S+)\s*(?:&&|;)\s*/;

/// 1 行目を見出し、残り（引数と結果）を畳んだ中身にする。よく出る `cd <場所> && ` は見出しから落とす。
export function splitStep(text) {
  const newline = text.indexOf('\n');
  const title = (newline < 0 ? text : text.slice(0, newline)).replace(CD, '').trim();
  const rest = (newline < 0 ? '' : text.slice(newline + 1)).split('\n').filter(line => !/^\s*```[\w-]*\s*$/.test(line)).join('\n').trim();
  return { title, rest };
}

/// パスを「名前」と「場所」に分ける（`docs/X.md (1 - 60)` → X.md / docs · 1 - 60。ホームは ~ に縮める）。
function file(argument) {
  const [, path = '', range = ''] = argument.match(/^(.*?)(?:\s+\(([^)]*)\))?$/) ?? [];
  const short = path.replace(/^["']|["']$/g, '').replace(/^\/Users\/[^/]+/, '~');
  const slash = short.lastIndexOf('/');
  return { target: slash < 0 ? short : short.slice(slash + 1), place: [slash < 0 ? '' : short.slice(0, slash), range].filter(Boolean).join(' · '), code: false };
}

export function describeStep(text) {
  const { title, rest } = splitStep(text);
  const space = title.indexOf(' ');
  const head = space < 0 ? title : title.slice(0, space);
  const argument = space < 0 ? '' : title.slice(space + 1).trim();
  const plain = (kind, target, place = '') => ({ kind, target, place, code: false, rest });
  const command = kind => ({ kind, target: title, place: '', code: true, rest });
  if (FILE_TOOLS[head]) {
    // 中身の 1 行目は同じファイルの絶対パス（引数）なので、見出しと重ねて見せない。
    const path = argument.replace(/\s+\([^)]*\)$/, '');
    const lines = rest.split('\n');
    const body = path && lines[0]?.startsWith('/') && lines[0].endsWith(path) ? lines.slice(1).join('\n').trim() : rest;
    // Codex の編集は `Editing files` で届く（対象は結果の側にある）。
    return { kind: FILE_TOOLS[head], ...(argument && argument !== 'files' ? file(argument) : { target: '', place: '', code: false }), rest: body };
  }
  if (head === 'Fetch' || head === 'WebFetch') return plain('web', argument);
  if (head === 'Search' || head === 'WebSearch') return plain('web', argument.replace(/^"(.*)"$/, '$1'));
  if (head === 'ToolSearch') return plain('tool', argument || head);
  if (head.startsWith('mcp__')) return plain('tool', head.slice(5).replace(/__/g, ' · '), argument);
  // ここから先はシェルのコマンド。最初の 1 本（`|` `&&` `;` の手前）で見分ける。
  const first = title.split(/\s*(?:\|\||\||&&|;)\s*/)[0];
  const words = first.split(/\s+/);
  const program = words[0];
  const redirect = first.match(/>>?\s*("[^"]+"|'[^']+'|[^\s;&|]+)/);
  if ((['cat', 'echo', 'printf'].includes(program) && redirect) || program === 'tee') {
    return { kind: 'write', ...file(redirect?.[1] ?? words.at(-1)), rest };
  }
  if (program === 'sed' && /\s-i\b/.test(first)) return command('edit');
  if (READERS.has(program)) {
    // 1 ファイルを読むだけの形（`sed -n 1,80p a/b.rs` / `cat a/b.rs`）は名前と場所で見せる。
    const last = words.at(-1);
    const range = program === 'sed' ? first.match(/-n\s+'?(\d+,\d+)p/)?.[1] : '';
    if (words.length > 1 && /[./]/.test(last) && !last.startsWith('-') && !/[*?$]/.test(last) && first === title) {
      const described = file(last);
      return { kind: 'read', ...described, place: [described.place, range].filter(Boolean).join(' · '), rest };
    }
    return command('read');
  }
  if (SEARCHERS.has(program) || /^git\s+grep\b/.test(first)) return command('search');
  if (program === 'curl' || program === 'wget') return command('web');
  return command('run');
}
