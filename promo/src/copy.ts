// 画面に出る言葉（日本語版 / 英語版）。事実の出所は README・LP・docs/GLOSSARY.md・locales。
// 性能は数値でなく「Rust × GPUI の速さ」で見せる（SSH の軽さだけは数字が一番伝わるので残す）。

export type Language = 'ja' | 'en';

export type Copy = {
  lost: {windows: string; agents: string; branches: string; prompt: string; wrong: string};
  built: {so: string; built: string; scratch: string; rust: string; slams: [string, string, string, string]};
  logo: {tagline: string; sub: string};
  color: {title: string; flow: string; labels: {rail: string; tab: string; caret: string; chip: string}; glance: string};
  agents: {
    title: string;
    sub: string;
    multi: string;
    multiSub: string;
    fleet: string;
    fleetSub: string;
    tasks: [string, string, string, string, string];
    captain: string;
    captainSub: string;
  };
  touches: {kicker: string; preview: string; previewSub: string; chat: string; chatSub: string};
  phone: {title: string; sub: string; points: [string, string, string, string]};
  ssh: {title: string; sub: string; points: [string, string, string, string]};
  power: {sub: string; items: [{value: string; label: string}, {value: string; label: string}, {value: string; label: string}]};
  extras: {title: string; chips: string[]};
  outro: {inside: string; insideSub: string; next: string; meta: string; credit: string};
};

export const copy: Record<Language, Copy> = {
  ja: {
    lost: {
      windows: 'ウィンドウ',
      agents: 'エージェント',
      branches: 'ブランチ',
      prompt: 'このブランチのテストを直して',
      wrong: '……送り先、違った。',
    },
    built: {
      so: 'だから、',
      built: 'エディタを作った。',
      scratch: 'ゼロから。',
      rust: 'Rust で。',
      slams: ['フォークじゃない。', 'Electron なし。', 'GPU で描く。', 'AI が中で動く。'],
    },
    logo: {tagline: '色で、迷わない。', sub: 'AI エージェント時代のコードエディタ'},
    color: {
      title: 'プロジェクトごとに、色がある。',
      flow: 'レール、タブ、キャレット。ぜんぶ同じ色。',
      labels: {rail: 'レール', tab: 'タブ', caret: 'キャレット', chip: '送り先'},
      glance: 'いまどこにいるか、ひと目で分かる。',
    },
    agents: {
      title: 'Claude Code も、Codex も。',
      sub: 'ACP 対応のエージェントが、エディタの中で動く。手持ちのサブスクのまま、API キーは要らない。',
      multi: 'タブで、\n何体でも並走。',
      multiSub: 'スレッドごとに色。送り先とトークンは、いつも見えている。',
      fleet: 'Fleet',
      fleetSub: '1 Task = 1 ブランチ = 1 worktree。エージェントを並べて走らせる。',
      tasks: ['認証リファクタ', 'バグ #412', 'LP 文言', '依存更新', 'リサーチ'],
      captain: 'Captain を任命すると、作業の分け方を提案してくる。',
      captainSub: '承認と統合は人間。Captain は自分でコードを書かない。',
    },
    touches: {
      kicker: 'かゆいところに、手が届く',
      preview: 'HTML も Markdown も、\nその場でプレビュー。',
      previewSub: '⌘⇧V で、ソースとプレビューを行き来する。',
      chat: 'コードじゃない相談は、\nChat モードで。',
      chatSub: '成果物は書類フォルダに、ふつうのファイルとして残る。',
    },
    phone: {
      title: 'スマホから、\nエージェントを動かす。',
      sub: '指示も、承認も、手元で。',
      points: ['QR を読むだけ', 'アカウント登録なし', '端末間で暗号化', 'Mac のポートは開けない'],
    },
    ssh: {
      title: 'SSH の先でも、\n同じ necoder。',
      sub: 'いつもの ~/.ssh/config と鍵が、そのまま使える。',
      points: ['サーバは 2.4MB の静的バイナリ', '待機時 6.5MB', '両側とも node 不要', 'ProxyJump もそのまま'],
    },
    power: {
      sub: 'GPU で直接描く、ネイティブの速さ。',
      items: [
        {value: '60fps', label: 'スクロールも入力も'},
        {value: '省メモリ', label: 'Electron を積まない'},
        {value: 'Native', label: 'Rust で、ゼロから'},
      ],
    },
    extras: {
      title: 'エディタの基本も、全部入り。',
      chips: ['LSP', 'tree-sitter', 'Git', 'ターミナル', '全体検索', 'マルチカーソル', '日本語 IME', 'diff レビュー', '巻き戻し', 'Todo ボード', 'テーマ', 'テレメトリなし'],
    },
    outro: {
      inside: 'necoder は、\nnecoder の中で作っている。',
      insideSub: '載せているエージェントと一緒に。',
      next: '次のエディタは、\nエージェントごと動く。',
      meta: '無料・オープンソース（AGPL-3.0）・macOS / Windows',
      credit: '映像は Remotion で描画、音楽は Rust で合成（録音素材なし）',
    },
  },
  en: {
    lost: {
      windows: 'windows',
      agents: 'agents',
      branches: 'branches',
      prompt: 'fix the tests on this branch',
      wrong: '…wrong window.',
    },
    built: {
      so: 'So I built',
      built: 'an editor.',
      scratch: 'From scratch.',
      rust: 'In Rust.',
      slams: ['No fork.', 'No Electron.', 'Drawn on the GPU.', 'Agents inside.'],
    },
    logo: {tagline: 'Orient by color.', sub: 'A code editor for the agent era'},
    color: {
      title: 'Every project gets a color.',
      flow: 'Rail. Tab. Caret. One color.',
      labels: {rail: 'rail', tab: 'tab', caret: 'caret', chip: 'destination'},
      glance: 'Know where you are. At a glance.',
    },
    agents: {
      title: 'Claude Code. Codex. And more.',
      sub: 'Any ACP agent runs inside the editor, on the subscription you already have. No API key.',
      multi: 'As many agents\nas you have tabs.',
      multiSub: 'Every thread has its color. Destination and tokens stay on screen.',
      fleet: 'Fleet',
      fleetSub: '1 task = 1 branch = 1 worktree. Run agents side by side.',
      tasks: ['Auth refactor', 'Bug #412', 'LP copy', 'Dependency bump', 'Research'],
      captain: 'Appoint a Captain. It proposes how to split the work.',
      captainSub: 'Approval and merging stay with you. The Captain never writes code.',
    },
    touches: {
      kicker: 'All the little things',
      preview: 'HTML and Markdown,\npreviewed in place.',
      previewSub: '⌘⇧V flips between source and preview.',
      chat: "For what isn't code:\nChat mode.",
      chatSub: 'Artifacts are plain files in your Documents folder.',
    },
    phone: {
      title: 'Drive your agents\nfrom your phone.',
      sub: 'Instruct, approve, follow along. From anywhere.',
      points: ['Scan one QR', 'No account', 'End-to-end encrypted', 'No open ports on your Mac'],
    },
    ssh: {
      title: 'The same necoder,\nover SSH.',
      sub: 'Your ~/.ssh/config and keys, as they are.',
      points: ['2.4MB static server', '6.5MB idle', 'No node on either side', 'ProxyJump just works'],
    },
    power: {
      sub: 'Drawn straight on the GPU. Native speed.',
      items: [
        {value: '60fps', label: 'scrolling and typing'},
        {value: 'Lean', label: 'no Electron inside'},
        {value: 'Native', label: 'Rust, from scratch'},
      ],
    },
    extras: {
      title: 'Everything you expect from an editor.',
      chips: ['LSP', 'tree-sitter', 'Git', 'Terminal', 'Project search', 'Multi-cursor', 'Japanese IME', 'Diff review', 'Rewind', 'Todo board', 'Themes', 'No telemetry'],
    },
    outro: {
      inside: 'necoder is built\ninside necoder.',
      insideSub: 'Together with the agents it hosts.',
      next: 'The next editor\nmoves with its agents.',
      meta: 'Free · Open source (AGPL-3.0) · macOS / Windows',
      credit: 'Rendered with Remotion. Music synthesized in Rust, no samples.',
    },
  },
};
