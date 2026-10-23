// 色とフォント。値はアプリ本体（docs/UI-SPEC.md §1・theme_core）から持ってくる。
// 動画も「色は識別にだけ使う」を守る: 地は無彩色、色が出るのはプロジェクト/スレッドを指すときだけ。

export const palette = {
  ink: '#07080b',
  bg0: '#16181e',
  bg1: '#1b1e25',
  bg2: '#232733',
  bg3: '#2b3040',
  fg0: '#d7dae3',
  fg1: '#9aa1b2',
  fg2: '#646b7a',
  border: '#262b36',
  white: '#f5f6f8',
  claude: '#d97757',
} as const;

export const syntax = {
  keyword: '#c678dd',
  function: '#61afef',
  type: '#e5c07b',
  string: '#98c379',
  number: '#d19a66',
  comment: '#5f6672',
  macro: '#56b6c2',
  punctuation: '#abb2bf',
  plain: '#d7dae3',
} as const;

// theme_core::IDENTITY_PALETTE_HEXES の巡回 5 色（red / green / blue / orange / violet）
export const projectColors = ['#ef4444', '#22c55e', '#3b82f6', '#f97316', '#a855f7'] as const;
// theme_core のスレッド色の巡回
export const threadColors = ['#61afef', '#e5c07b', '#c678dd'] as const;

export const projects = [
  {name: 'necoder', branch: 'main', monogram: 'n', color: projectColors[0]},
  {name: 'api', branch: 'fix/auth', monogram: 'a', color: projectColors[1]},
  {name: 'web', branch: 'feat/landing', monogram: 'w', color: projectColors[2]},
  {name: 'infra', branch: 'main', monogram: 'i', color: projectColors[3]},
  {name: 'ml', branch: 'exp/rerank', monogram: 'm', color: projectColors[4]},
] as const;

export const fonts = {
  display: '"Archivo", "IBM Plex Sans JP", sans-serif',
  japanese: '"IBM Plex Sans JP", "Archivo", sans-serif',
  mono: '"Guguru Sans Code", "IBM Plex Sans JP", monospace',
} as const;
