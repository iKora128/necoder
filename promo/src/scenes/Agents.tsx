// 32–48 秒: AI。エージェント一覧 → タブで何体でも（送り先とトークン）→ Fleet → Captain の任命と分解案。
// 見た目の寸法・文言はアプリ（UI-SPEC §6・locales）に合わせて組み直している。
import React from 'react';
import {AbsoluteFill, Easing, interpolate, Sequence, spring, staticFile, useCurrentFrame, useVideoConfig} from 'remotion';
import {Caption} from '../components/Caption';
import {Mascot} from '../components/Mascot';
import {Fade, Rise} from '../components/Text';
import {copy, type Language} from '../copy';
import {fonts, palette, projects, threadColors} from '../theme';
import {beats, STEP} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const AGENTS: {label: string; icon?: string; monogram?: string; tint: string}[] = [
  {label: 'Claude Code', icon: 'icons/brand-claude.svg', tint: palette.claude},
  {label: 'Codex', monogram: '>_', tint: palette.fg0},
  {label: 'GitHub Copilot', icon: 'icons/brand-copilot.svg', tint: palette.fg0},
  {label: 'Qwen Code', icon: 'icons/brand-qwen.svg', tint: palette.fg0},
  {label: 'OpenCode', icon: 'icons/brand-opencode.svg', tint: palette.fg0},
  {label: 'Kimi CLI', icon: 'icons/brand-kimi.svg', tint: palette.fg0},
  {label: 'Grok Build', monogram: 'G', tint: palette.fg0},
];

const Icon: React.FC<{src: string; size: number; color: string}> = ({src, size, color}) => (
  <div
    style={{
      width: size,
      height: size,
      background: color,
      WebkitMaskImage: `url(${staticFile(src)})`,
      maskImage: `url(${staticFile(src)})`,
      WebkitMaskSize: 'contain',
      maskSize: 'contain',
      WebkitMaskRepeat: 'no-repeat',
      maskRepeat: 'no-repeat',
      WebkitMaskPosition: 'center',
      maskPosition: 'center',
    }}
  />
);

// ── 1. エージェント一覧 ───────────────────────────────────────────────
const AgentTiles: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].agents;
  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <div style={{position: 'absolute', top: 150, width: '100%', textAlign: 'center'}}>
        <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 88, color: palette.white}}>
          <Rise text={text.title} at={0} />
        </div>
        <Fade at={beats(2)} style={{fontFamily: fonts.japanese, fontSize: 32, color: palette.fg1, marginTop: 22}}>
          {text.sub}
        </Fade>
      </div>
      <div style={{position: 'absolute', top: 520, width: '100%', display: 'flex', justifyContent: 'center', gap: 26}}>
        {AGENTS.map((agent, index) => {
          const at = beats(0.5) + index * STEP * 2;
          const enter = spring({frame: frame - at, fps, config: {damping: 15, stiffness: 200, mass: 0.7}});
          return (
            <div
              key={agent.label}
              style={{
                width: 196,
                height: 214,
                borderRadius: 26,
                background: palette.bg0,
                border: `1px solid ${index === 0 ? `${palette.claude}88` : palette.border}`,
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                justifyContent: 'center',
                gap: 24,
                opacity: frame >= at ? Math.min(1, enter * 1.5) : 0,
                transform: `translateY(${(1 - enter) * 60}px) scale(${0.9 + 0.1 * enter})`,
                boxShadow: '0 24px 60px rgba(0,0,0,0.5)',
              }}
            >
              {agent.icon ? (
                <Icon src={agent.icon} size={84} color={agent.tint} />
              ) : (
                <div style={{fontFamily: fonts.mono, fontWeight: 700, fontSize: 64, color: agent.tint, height: 84, lineHeight: '84px'}}>{agent.monogram}</div>
              )}
              <div style={{fontFamily: fonts.display, fontWeight: 600, fontSize: 21, color: palette.fg0}}>{agent.label}</div>
            </div>
          );
        })}
      </div>
      <Fade at={beats(4.5)} style={{position: 'absolute', top: 800, width: '100%', textAlign: 'center'}}>
        <span style={{fontFamily: fonts.mono, fontSize: 30, color: palette.fg1, border: `1px solid ${palette.border}`, borderRadius: 999, padding: '12px 30px'}}>
          ACP · Agent Client Protocol
        </span>
      </Fade>
    </AbsoluteFill>
  );
};

// ── 2. タブで何体でも（Editor モードの AI スレッド）─────────────────────
// スレッドはアクティブプロジェクトに属する（UI-SPEC §6）。だから 3 本とも necoder ⎇ main。
type AgentId = 'claude' | 'codex' | 'copilot';
type ThreadDemo = {name: string; agent: AgentId; tokens: number; question: string; tool: string; argument: string; result: string; answer: string; lines: [string, string]};
const THREADS: Record<Language, ThreadDemo[]> = {
  ja: [
    {name: 'rope 設計', agent: 'claude', tokens: 23.4, question: 'MVP のバッファ、ropey と sum-tree どっちに寄せる？', tool: 'Read', argument: 'crates/editor_core/src/editor_core.rs', result: '214 行', answer: '結論: MVP は ropey', lines: ['ropey は API が安定・docs も厚い', 'Buffer を trait で切れば、後から差し替えられる']},
    {name: 'tab 色分け', agent: 'codex', tokens: 9.1, question: 'タブの色、どう使うのがいい？', tool: 'Search', argument: 'project_color', result: '12 件', answer: '結論: 色は識別だけに使う', lines: ['状態は色相でなく、形と動きで出す', 'スレッドの色は 3 色で巡回させる']},
    {name: 'LP 文言', agent: 'copilot', tokens: 6.8, question: 'ヒーローの見出しを 2 案ちょうだい', tool: 'Read', argument: 'lp/i18n/en.json', result: '98 行', answer: 'A: 色で、迷わない。', lines: ['B: エディタは、エージェントごと動く。', 'どちらも 1 行に収まります']},
  ],
  en: [
    {name: 'rope design', agent: 'claude', tokens: 23.4, question: 'For the MVP buffer: ropey or sum-tree?', tool: 'Read', argument: 'crates/editor_core/src/editor_core.rs', result: '214 lines', answer: 'Verdict: ropey for the MVP', lines: ['stable API and solid docs', 'a Buffer trait keeps it swappable']},
    {name: 'tab colors', agent: 'codex', tokens: 9.1, question: 'How should tabs use color?', tool: 'Search', argument: 'project_color', result: '12 matches', answer: 'Verdict: color marks identity only', lines: ['state uses shape and motion, not hue', 'thread colors cycle through three']},
    {name: 'LP copy', agent: 'copilot', tokens: 6.8, question: 'Give me two hero headlines', tool: 'Read', argument: 'lp/i18n/en.json', result: '98 lines', answer: 'A: Orient by color.', lines: ['B: The editor moves with its agents.', 'both fit on one line']},
  ],
};
const AGENT_LABEL: Record<AgentId, string> = {claude: 'Claude Code', codex: 'Codex', copilot: 'GitHub Copilot'};

const AgentMark: React.FC<{agent: AgentId; size: number}> = ({agent, size}) =>
  agent === 'codex' ? (
    <span style={{fontFamily: fonts.mono, fontWeight: 700, fontSize: size * 0.78, color: palette.fg0, width: size, textAlign: 'center', lineHeight: 1}}>&gt;_</span>
  ) : (
    <Icon src={agent === 'claude' ? 'icons/brand-claude.svg' : 'icons/brand-copilot.svg'} size={size} color={agent === 'claude' ? palette.claude : palette.fg0} />
  );

const streamed = (text: string, frame: number, at: number, charactersPerFrame: number) => {
  const count = Math.max(0, Math.floor((frame - at) * charactersPerFrame));
  return Array.from(text).slice(0, count).join('');
};

const MultiAgent: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const text = copy[language].agents;
  const threads = THREADS[language];
  const active = Math.floor(frame / beats(2)) % threads.length;
  const since = frame % beats(2);
  const thread = threads[active];
  const color = threadColors[active];
  const project = projects[0];
  const panelIn = interpolate(frame, [0, 16], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  const swap = interpolate(since, [0, 10], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  const tokens = thread.tokens * interpolate(since, [0, 50], [0.92, 1], clamp);
  const total = threads.reduce((sum, entry) => sum + entry.tokens, 0);
  const rowStyle: React.CSSProperties = {fontFamily: fonts.japanese, fontSize: 25, color: palette.fg0, lineHeight: 1.65};
  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <div
        style={{
          position: 'absolute',
          left: 120,
          top: 90,
          width: 1060,
          height: 900,
          borderRadius: 18,
          background: palette.bg1,
          border: `2px solid ${project.color}`,
          overflow: 'hidden',
          boxShadow: '0 40px 120px rgba(0,0,0,0.7)',
          opacity: panelIn,
          transform: `translateY(${(1 - panelIn) * 40}px)`,
        }}
      >
        <div style={{height: 70, display: 'flex', alignItems: 'stretch', background: palette.bg0, borderBottom: `1px solid ${palette.border}`, fontFamily: fonts.japanese, fontSize: 23}}>
          <div style={{display: 'flex', alignItems: 'center', padding: '0 22px', color: palette.fg1, fontFamily: fonts.mono}}>Σ {total.toFixed(1)}k</div>
          {threads.map((entry, index) => (
            <div
              key={entry.name}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 12,
                padding: '0 24px',
                color: index === active ? palette.white : palette.fg1,
                background: index === active ? palette.bg1 : 'transparent',
                borderTop: `3px solid ${index === active ? threadColors[index] : 'transparent'}`,
              }}
            >
              <div style={{width: 11, height: 11, borderRadius: 6, background: threadColors[index]}} />
              <AgentMark agent={entry.agent} size={22} />
              {entry.name}
            </div>
          ))}
          <div style={{display: 'flex', alignItems: 'center', padding: '0 20px', color: palette.fg2, fontSize: 30}}>+</div>
        </div>
        <div style={{height: 118, display: 'flex', alignItems: 'center', padding: '0 28px', borderBottom: `1px solid ${palette.border}`, gap: 24}}>
          <div style={{display: 'flex', alignItems: 'center', gap: 12, fontFamily: fonts.display, fontSize: 24, color: palette.fg0}}>
            <AgentMark agent={thread.agent} size={26} />
            {AGENT_LABEL[thread.agent]}
          </div>
          <div style={{flex: 1}} />
          <Mascot kind="typing" scale={1.45} />
          <div style={{display: 'flex', alignItems: 'center', gap: 16}}>
            <div style={{width: 170, height: 7, borderRadius: 4, background: palette.bg3, overflow: 'hidden'}}>
              <div style={{width: `${(tokens / 200) * 100}%`, height: '100%', background: color}} />
            </div>
            <span style={{fontFamily: fonts.mono, fontSize: 22, color: palette.fg1}}>{tokens.toFixed(1)}k/200k</span>
          </div>
        </div>
        <div style={{padding: '28px 30px', display: 'flex', flexDirection: 'column', gap: 20, opacity: swap, transform: `translateY(${(1 - swap) * 14}px)`}}>
          <div style={{...rowStyle, background: palette.bg2, borderLeft: `3px solid ${color}`, borderRadius: 8, padding: '14px 20px'}}>{thread.question}</div>
          <div style={{opacity: interpolate(since, [4, 10], [0, 1], clamp)}}>
            <div style={{...rowStyle, display: 'flex', alignItems: 'center', gap: 12}}>
              <span style={{color: palette.claude, fontSize: 18}}>⏺</span>
              <b style={{fontWeight: 700}}>{thread.tool}</b>
              <span style={{fontFamily: fonts.mono, color: palette.fg2, fontSize: 21}}>{thread.argument}</span>
            </div>
            <div style={{fontFamily: fonts.mono, fontSize: 20, color: palette.fg2, paddingLeft: 30}}>⎿ {thread.result}</div>
          </div>
          <div style={{...rowStyle, fontWeight: 700, fontSize: 32, color: palette.white, marginTop: 4}}>{streamed(thread.answer, since, 12, 0.9)}</div>
          {thread.lines.map((line, index) => (
            <div key={line} style={{...rowStyle, display: 'flex', gap: 14}}>
              <span style={{color: palette.fg2, opacity: since > 24 + index * 10 ? 1 : 0}}>•</span>
              <span>{streamed(line, since, 24 + index * 10, 1.4)}</span>
            </div>
          ))}
        </div>
        <div
          style={{
            position: 'absolute',
            left: 26,
            right: 26,
            bottom: 26,
            height: 170,
            borderRadius: 14,
            border: `2px solid ${color}88`,
            background: palette.bg1,
            padding: '20px 24px',
            boxSizing: 'border-box',
          }}
        >
          <div style={{display: 'flex', alignItems: 'center', gap: 12, fontFamily: fonts.japanese, fontSize: 23, color: palette.fg0}}>
            <div style={{width: 11, height: 11, borderRadius: 6, background: color}} />
            {thread.name}
            <div style={{flex: 1}} />
            <span style={{color: palette.fg1}}>
              {project.name} <span style={{fontFamily: fonts.mono}}>⎇ {project.branch}</span>
            </span>
          </div>
          <div style={{display: 'flex', justifyContent: 'flex-end', marginTop: 54}}>
            <div style={{background: color, color: palette.ink, fontWeight: 700, fontFamily: fonts.japanese, fontSize: 21, borderRadius: 8, padding: '8px 18px'}}>
              {language === 'en' ? 'Send ⌘↵' : '送信 ⌘↵'}
            </div>
          </div>
        </div>
      </div>
      <div style={{position: 'absolute', left: 1260, top: 300, width: 600}}>
        <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 64, lineHeight: 1.28, color: palette.white}}>
          <Rise text={text.multi} at={6} />
        </div>
        <Fade at={beats(1.5)} style={{fontFamily: fonts.japanese, fontSize: 28, lineHeight: 1.6, color: palette.fg1, marginTop: 26}}>
          {text.multiSub}
        </Fade>
      </div>
    </AbsoluteFill>
  );
};

// ── 3. Fleet（Task の一覧と系譜）──────────────────────────────────────
const BRAILLE = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

type Phase = 'working' | 'blocked' | 'ready' | 'failed' | 'planned';
const TASKS: {branch: string; phase: Phase; tokens: string}[] = [
  {branch: 'task/auth', phase: 'working', tokens: '42.1k'},
  {branch: 'task/fix-412', phase: 'blocked', tokens: '18.7k'},
  {branch: 'task/lp-copy', phase: 'ready', tokens: '9.3k'},
  {branch: 'task/deps', phase: 'failed', tokens: '6.0k'},
  {branch: 'task/research', phase: 'planned', tokens: '—'},
];
const PHASE_LABEL: Record<Language, Record<Phase, string>> = {
  ja: {working: '実行中', blocked: '承認待ち', ready: 'マージ可', failed: '失敗', planned: '予定'},
  en: {working: 'running', blocked: 'needs approval', ready: 'ready to merge', failed: 'failed', planned: 'planned'},
};

/** 状態は色相ではなく形と動きで出す（UI-SPEC §1.3）。色はスレッドの識別色。 */
const StateGlyph: React.FC<{phase: Phase; color: string; size: number}> = ({phase, color, size}) => {
  const frame = useCurrentFrame();
  const box: React.CSSProperties = {width: size, height: size, display: 'flex', alignItems: 'center', justifyContent: 'center'};
  if (phase === 'working') {
    return (
      <div style={{...box, fontFamily: fonts.mono, fontSize: size * 1.05, color, lineHeight: 1}}>{BRAILLE[Math.floor(frame / 7.5) % BRAILLE.length]}</div>
    );
  }
  if (phase === 'blocked') {
    const pulse = 0.55 + 0.45 * Math.abs(Math.sin(frame / 6));
    return (
      <div style={box}>
        <div style={{width: size * 0.7, height: size * 0.7, borderRadius: '50%', border: `${size * 0.1}px solid ${color}`, background: `linear-gradient(90deg, ${color} 50%, transparent 50%)`, opacity: pulse}} />
      </div>
    );
  }
  if (phase === 'ready') {
    return (
      <div style={box}>
        <div style={{width: size * 0.7, height: size * 0.7, borderRadius: '50%', border: `${size * 0.12}px solid ${color}`}} />
      </div>
    );
  }
  if (phase === 'failed') {
    return (
      <div style={box}>
        <div style={{width: size * 0.7, height: size * 0.7, borderRadius: '50%', border: `${size * 0.1}px dashed ${color}`}} />
      </div>
    );
  }
  return (
    <div style={box}>
      <div style={{width: size * 0.36, height: size * 0.36, borderRadius: '50%', background: color, opacity: 0.35}} />
    </div>
  );
};

const Fleet: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const text = copy[language].agents;
  const base = {x: 980, y: 600};
  const targets = TASKS.map((_, index) => ({x: 1560, y: 330 + index * 135}));
  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <div style={{position: 'absolute', left: 140, top: 96}}>
        <div style={{fontFamily: fonts.display, fontWeight: 800, fontSize: 120, color: palette.white, lineHeight: 1}}>
          <Rise text={text.fleet} at={0} />
        </div>
        <Fade at={beats(1)} style={{fontFamily: fonts.japanese, fontSize: 30, color: palette.fg1, marginTop: 22}}>
          {text.fleetSub}
        </Fade>
      </div>

      {/* 左: Task の一覧 */}
      <div style={{position: 'absolute', left: 140, top: 350, width: 640, display: 'flex', flexDirection: 'column', gap: 14}}>
        {TASKS.map((task, index) => {
          const at = beats(0.5) + index * STEP * 2;
          const enter = spring({frame: frame - at, fps, config: {damping: 200, stiffness: 160}});
          const color = threadColors[index % threadColors.length];
          return (
            <div
              key={task.branch}
              style={{
                height: 104,
                borderRadius: 14,
                background: palette.bg0,
                border: `1px solid ${palette.border}`,
                display: 'flex',
                alignItems: 'center',
                gap: 22,
                padding: '0 26px',
                opacity: frame >= at ? enter : 0,
                transform: `translateX(${(1 - enter) * -50}px)`,
              }}
            >
              <StateGlyph phase={task.phase} color={color} size={34} />
              <div style={{flex: 1}}>
                <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 30, color: palette.white}}>{text.tasks[index]}</div>
                <div style={{fontFamily: fonts.mono, fontSize: 21, color: palette.fg1, marginTop: 2}}>⎇ {task.branch}</div>
              </div>
              <div style={{textAlign: 'right'}}>
                <div style={{fontFamily: fonts.japanese, fontSize: 22, color: palette.fg1}}>{PHASE_LABEL[language][task.phase]}</div>
                <div style={{fontFamily: fonts.mono, fontSize: 20, color: palette.fg2}}>{task.tokens}</div>
              </div>
            </div>
          );
        })}
      </div>

      {/* 右: 系譜（扇形）。main から Task の worktree が伸びる */}
      <svg width={1920} height={1080} style={{position: 'absolute', inset: 0}}>
        {targets.map((target, index) => {
          const at = beats(1) + index * STEP * 2;
          const progress = interpolate(frame, [at, at + 26], [0, 1], {...clamp, easing: Easing.inOut(Easing.cubic)});
          const color = threadColors[index % threadColors.length];
          const path = `M ${base.x} ${base.y} C ${base.x + 260} ${base.y}, ${target.x - 300} ${target.y}, ${target.x} ${target.y}`;
          return (
            <path key={index} d={path} stroke={color} strokeWidth={4} fill="none" pathLength={1} strokeDasharray={1} strokeDashoffset={1 - progress} strokeLinecap="round" opacity={0.9} />
          );
        })}
      </svg>
      <div style={{position: 'absolute', left: base.x - 44, top: base.y - 44, width: 88, height: 88, borderRadius: 44, border: `5px solid ${projects[0].color}`, background: palette.bg0, display: 'flex', alignItems: 'center', justifyContent: 'center', boxSizing: 'border-box'}}>
        <span style={{fontFamily: fonts.mono, fontSize: 20, color: palette.white}}>main</span>
      </div>
      {targets.map((target, index) => {
        const at = beats(1) + index * STEP * 2 + 22;
        const enter = spring({frame: frame - at, fps, config: {damping: 14, stiffness: 220}});
        const color = threadColors[index % threadColors.length];
        return (
          <div key={index} style={{position: 'absolute', left: target.x - 26, top: target.y - 26, display: 'flex', alignItems: 'center', gap: 18, opacity: frame >= at ? 1 : 0, transform: `scale(${enter})`, transformOrigin: '26px 26px'}}>
            <div style={{width: 52, height: 52, borderRadius: 26, background: palette.bg0, border: `3px solid ${color}`, display: 'flex', alignItems: 'center', justifyContent: 'center', boxSizing: 'border-box'}}>
              <StateGlyph phase={TASKS[index].phase} color={color} size={30} />
            </div>
            <span style={{fontFamily: fonts.mono, fontSize: 22, color: palette.fg1}}>{TASKS[index].branch}</span>
          </div>
        );
      })}
    </AbsoluteFill>
  );
};

// ── 4. Captain の任命 → 分解案 ──────────────────────────────────────
// 任命の文言は locales の captain.appoint_*、分解案は captain.proposal_* をそのまま使う。
const APPOINT = {
  ja: {
    title: 'Captain を任命する',
    body: 'Captain は main に住む采配役です。目標を渡すと Task（worktree）に分けて起動し、終わり・失敗・承認待ちを見張って報告します。自分ではコードを書きません。どのエージェントでも務まり、あとで設定から交代・解任できます。',
    gate: '采配のみ · 統合と承認は人間が操作',
  },
  en: {
    title: 'Appoint a Captain',
    body: 'The Captain is a coordinator that lives on main. Give it a goal and it splits the work into Tasks (worktrees), starts them, watches for done / failed / waiting, and reports back. It never writes code itself. Any agent can serve, and you can replace or dismiss it later in Settings.',
    gate: 'Coordination only · Integration and approvals require a human',
  },
} as const;

const PROPOSAL = {
  ja: {
    title: '⚑ Captain の分解案 · 3 本',
    rows: [
      {title: '認証リファクタ', goal: '目的: セッション検証を 1 か所に寄せる', done: '完了: cargo test -p api が通る', agent: 'Claude Code'},
      {title: 'バグ #412', goal: '目的: IME 確定時のキャレットずれを直す', done: '完了: 再現テストが緑', agent: 'Codex'},
      {title: 'LP 文言', goal: '目的: ヒーロー節を ja / en で書き直す', done: '完了: lp-build の警告が 0', agent: 'Claude Code'},
    ],
    approve: '承認（3 本を切る）',
    reject: '却下',
  },
  en: {
    title: "⚑ Captain's split · 3 Tasks",
    rows: [
      {title: 'Auth refactor', goal: 'Goal: one place for session checks', done: 'Done when: cargo test -p api passes', agent: 'Claude Code'},
      {title: 'Bug #412', goal: 'Goal: fix the caret drift on IME commit', done: 'Done when: the repro test is green', agent: 'Codex'},
      {title: 'LP copy', goal: 'Goal: rewrite the hero section in ja / en', done: 'Done when: lp-build has zero warnings', agent: 'Claude Code'},
    ],
    approve: 'Approve (create 3)',
    reject: 'Reject',
  },
} as const;

const Cursor: React.FC<{x: number; y: number; visible: boolean}> = ({x, y, visible}) => (
  <svg width={40} height={40} viewBox="0 0 24 24" style={{position: 'absolute', left: x, top: y, opacity: visible ? 1 : 0, filter: 'drop-shadow(0 4px 10px rgba(0,0,0,0.6))'}}>
    <path d="M3 2 L3 19 L8 14.5 L11.5 22 L14.5 20.6 L11 13.2 L17.5 13.2 Z" fill={palette.white} stroke={palette.ink} strokeWidth={1.2} />
  </svg>
);

const Captain: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const {fps} = useVideoConfig();
  const appoint = APPOINT[language];
  const content = PROPOSAL[language];
  const text = copy[language].agents;

  // 任命（0–72）: Claude Code を選ぶ
  const appointIn = spring({frame, fps, config: {damping: 200, stiffness: 140}});
  const appointOut = interpolate(frame, [62, 74], [0, 1], {...clamp, easing: Easing.in(Easing.cubic)});
  const pickAt = 52;
  const picked = frame >= pickAt;
  const pickPress = interpolate(frame, [pickAt, pickAt + 4, pickAt + 10], [1, 0.94, 1], clamp);

  // 分解案（66–240）: 承認を押す
  const cardIn = spring({frame: frame - 66, fps, config: {damping: 200, stiffness: 120}});
  const pressAt = beats(5);
  const press = interpolate(frame, [pressAt, pressAt + 4, pressAt + 10], [1, 0.94, 1], clamp);
  const approved = frame >= pressAt + 4;

  const cursor =
    frame < 74
      ? {
          x: interpolate(frame, [8, pickAt - 4], [1500, 610], {...clamp, easing: Easing.inOut(Easing.cubic)}),
          y: interpolate(frame, [8, pickAt - 4], [980, 590], {...clamp, easing: Easing.inOut(Easing.cubic)}),
        }
      : {
          x: interpolate(frame, [beats(3.6), pressAt], [1640, 1330], {...clamp, easing: Easing.inOut(Easing.cubic)}),
          y: interpolate(frame, [beats(3.6), pressAt], [1000, 752], {...clamp, easing: Easing.inOut(Easing.cubic)}),
        };

  return (
    <AbsoluteFill style={{background: palette.ink}}>
      {frame < 76 ? (
        <div
          style={{
            position: 'absolute',
            left: 420,
            top: 150,
            width: 1080,
            borderRadius: 18,
            background: palette.bg0,
            border: `1px solid ${palette.border}`,
            boxShadow: '0 40px 120px rgba(0,0,0,0.7)',
            padding: '38px 44px',
            boxSizing: 'border-box',
            opacity: appointIn * (1 - appointOut),
            transform: `translateY(${(1 - appointIn) * 40 - appointOut * 30}px) scale(${1 - appointOut * 0.04})`,
          }}
        >
          <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 38, color: palette.white}}>⚑ {appoint.title}</div>
          <div style={{fontFamily: fonts.japanese, fontSize: 25, lineHeight: 1.75, color: palette.fg1, marginTop: 18}}>{appoint.body}</div>
          <div style={{display: 'flex', gap: 16, marginTop: 30}}>
            {(['claude', 'codex', 'copilot'] as AgentId[]).map((agent, index) => (
              <div
                key={agent}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 14,
                  padding: '16px 26px',
                  borderRadius: 12,
                  background: index === 0 && picked ? palette.bg3 : palette.bg1,
                  border: `2px solid ${index === 0 && picked ? palette.white : palette.border}`,
                  fontFamily: fonts.display,
                  fontWeight: 600,
                  fontSize: 25,
                  color: palette.fg0,
                  transform: index === 0 ? `scale(${pickPress})` : 'none',
                }}
              >
                <AgentMark agent={agent} size={28} />
                {AGENT_LABEL[agent]}
              </div>
            ))}
          </div>
          <div style={{fontFamily: fonts.japanese, fontSize: 21, color: palette.fg2, marginTop: 24}}>{appoint.gate}</div>
        </div>
      ) : null}

      {frame >= 66 ? (
        <div
          style={{
            position: 'absolute',
            left: 360,
            top: 110,
            width: 1200,
            borderRadius: 18,
            background: palette.bg0,
            border: `1px solid ${palette.border}`,
            boxShadow: '0 40px 120px rgba(0,0,0,0.7)',
            padding: '34px 40px',
            boxSizing: 'border-box',
            opacity: cardIn,
            transform: `translateY(${(1 - cardIn) * 50}px)`,
          }}
        >
          <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 34, color: palette.white}}>{content.title}</div>
          <div style={{display: 'flex', flexDirection: 'column', gap: 14, marginTop: 26}}>
            {content.rows.map((row, index) => {
              const at = 80 + index * STEP * 2;
              const enter = interpolate(frame, [at, at + 12], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
              return (
                <div
                  key={row.title}
                  style={{
                    display: 'flex',
                    alignItems: 'flex-start',
                    gap: 20,
                    padding: '18px 22px',
                    borderRadius: 12,
                    background: approved ? palette.bg2 : palette.bg1,
                    border: `1px solid ${palette.border}`,
                    opacity: enter,
                    transform: `translateY(${(1 - enter) * 16}px)`,
                  }}
                >
                  <div style={{fontFamily: fonts.mono, fontSize: 30, color: palette.white, lineHeight: 1.2}}>☑</div>
                  <div style={{flex: 1}}>
                    <div style={{display: 'flex', alignItems: 'baseline', gap: 16}}>
                      <span style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 29, color: palette.white}}>{row.title}</span>
                      <span style={{fontFamily: fonts.display, fontSize: 21, color: row.agent === 'Claude Code' ? palette.claude : palette.fg1}}>{row.agent}</span>
                    </div>
                    <div style={{fontFamily: fonts.japanese, fontSize: 23, color: palette.fg1, marginTop: 4}}>{row.goal}</div>
                    <div style={{fontFamily: fonts.japanese, fontSize: 23, color: palette.fg2}}>{row.done}</div>
                  </div>
                </div>
              );
            })}
          </div>
          <div style={{display: 'flex', justifyContent: 'flex-end', gap: 16, marginTop: 26}}>
            <div style={{fontFamily: fonts.japanese, fontSize: 25, color: palette.fg1, border: `1px solid ${palette.border}`, borderRadius: 10, padding: '12px 26px'}}>{content.reject}</div>
            <div
              style={{
                fontFamily: fonts.japanese,
                fontWeight: 700,
                fontSize: 25,
                color: palette.ink,
                background: palette.white,
                borderRadius: 10,
                padding: '12px 26px',
                transform: `scale(${press})`,
                boxShadow: approved
                  ? `0 0 0 ${interpolate(frame, [pressAt + 4, pressAt + 24], [0, 26], clamp)}px rgba(255,255,255,${interpolate(frame, [pressAt + 4, pressAt + 24], [0.35, 0], clamp)})`
                  : 'none',
              }}
            >
              {approved ? '✓ ' : ''}
              {content.approve}
            </div>
          </div>
        </div>
      ) : null}

      <Cursor x={cursor.x} y={cursor.y} visible={(frame > 8 && frame < 70) || frame > beats(3.6)} />
      <Caption text={text.captain} at={beats(0.5)} sub={text.captainSub} subAt={beats(2)} size={50} />
    </AbsoluteFill>
  );
};

export const Agents: React.FC<{language: Language}> = ({language}) => (
  <AbsoluteFill style={{background: palette.ink}}>
    <Sequence durationInFrames={beats(8)}>
      <AgentTiles language={language} />
    </Sequence>
    <Sequence from={beats(8)} durationInFrames={beats(8)}>
      <MultiAgent language={language} />
    </Sequence>
    <Sequence from={beats(16)} durationInFrames={beats(8)}>
      <Fleet language={language} />
    </Sequence>
    <Sequence from={beats(24)} durationInFrames={beats(8)}>
      <Captain language={language} />
    </Sequence>
  </AbsoluteFill>
);
