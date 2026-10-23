// 0–8 秒: 迷子。同じ見た目のウィンドウが 20 枚並び、エージェントが走り、送り先を間違える。
// ここだけは色を使わない（識別が無い世界）。
import React from 'react';
import {AbsoluteFill, Easing, interpolate, random, spring, useCurrentFrame, useVideoConfig} from 'remotion';
import {Rise, Typewriter} from '../components/Text';
import {copy, type Language} from '../copy';
import {fonts, palette} from '../theme';
import {cues, seconds, STEP} from '../timeline';

const clamp = {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'} as const;

const COLUMNS = 5;
const ROWS = 4;
const WINDOW_WIDTH = 278;
const WINDOW_HEIGHT = 164;
const GAP = 26;
const GRID_WIDTH = COLUMNS * WINDOW_WIDTH + (COLUMNS - 1) * GAP;
const GRID_HEIGHT = ROWS * WINDOW_HEIGHT + (ROWS - 1) * GAP;

const AGENT_WINDOWS = [2, 6, 9, 13, 17];
const BRANCH_WINDOWS: Record<number, string> = {1: 'main', 8: 'fix/auth', 15: 'feat/x'};
const TARGET_WINDOW = 7; // 送ったつもりの窓
const WRONG_WINDOW = 12; // 実際に届いた窓

const typing = cues.typing[0];
const TYPE_AT = seconds(typing.start);
const TYPE_INTERVAL = seconds(typing.interval);
const SEND_AT = seconds(6.25);
const ERROR_AT = seconds(cues.error[0]);

const windowPosition = (index: number) => {
  const column = index % COLUMNS;
  const row = Math.floor(index / COLUMNS);
  return {
    left: (1920 - GRID_WIDTH) / 2 + column * (WINDOW_WIDTH + GAP),
    top: 200 + row * (WINDOW_HEIGHT + GAP),
  };
};

const GrayWindow: React.FC<{index: number; frame: number}> = ({index, frame}) => {
  const {fps} = useVideoConfig();
  const appear = Math.round(index * STEP);
  const enter = spring({frame: frame - appear, fps, config: {damping: 200, stiffness: 220}});
  const isAgent = AGENT_WINDOWS.includes(index);
  const agentOrder = AGENT_WINDOWS.indexOf(index);
  const agentStart = seconds(2.5) + agentOrder * 12;
  const branch = BRANCH_WINDOWS[index];
  const branchIndex = Object.keys(BRANCH_WINDOWS).map(Number).indexOf(index);
  const branchAt = seconds(3.5) + branchIndex * 15;
  const lines = 6 + Math.floor(random(`lines-${index}`) * 3);
  const hit = index === WRONG_WINDOW ? interpolate(frame, [ERROR_AT, ERROR_AT + 2, ERROR_AT + 14], [0, 1, 0], clamp) : 0;
  // 送ったつもりの窓には、打っている間だけ薄い枠を出す
  const intended = index === TARGET_WINDOW ? interpolate(frame, [TYPE_AT - 12, TYPE_AT, SEND_AT, SEND_AT + 8], [0, 1, 1, 0], clamp) : 0;
  const fadeOut = interpolate(frame, [seconds(7.0) + random(`out-${index}`) * 40, seconds(7.0) + random(`out-${index}`) * 40 + 16], [1, 0], clamp);
  const {left, top} = windowPosition(index);
  return (
    <div
      style={{
        position: 'absolute',
        left,
        top,
        width: WINDOW_WIDTH,
        height: WINDOW_HEIGHT,
        borderRadius: 9,
        background: '#111318',
        border: `1px solid ${hit > 0 ? `rgba(255,255,255,${0.2 + hit * 0.8})` : intended > 0 ? `rgba(255,255,255,${0.15 + intended * 0.55})` : '#23262e'}`,
        boxShadow: hit > 0 ? `0 0 ${40 * hit}px rgba(255,255,255,${0.35 * hit})` : '0 12px 30px rgba(0,0,0,0.45)',
        opacity: enter * fadeOut,
        transform: `scale(${0.86 + 0.14 * enter})`,
        overflow: 'hidden',
      }}
    >
      <div style={{height: 20, display: 'flex', alignItems: 'center', gap: 5, padding: '0 8px', borderBottom: '1px solid #1d2027'}}>
        {[0, 1, 2].map((dot) => (
          <div key={dot} style={{width: 7, height: 7, borderRadius: 4, background: '#2c3039'}} />
        ))}
        <div
          style={{
            marginLeft: 8,
            fontFamily: fonts.mono,
            fontSize: 10,
            color: palette.fg2,
            opacity: branch ? interpolate(frame, [branchAt, branchAt + 8], [0, 1], clamp) : 0,
          }}
        >
          {branch ? `⎇ ${branch}` : ''}
        </div>
      </div>
      <div style={{display: 'flex', height: WINDOW_HEIGHT - 20}}>
        <div style={{width: 34, background: '#0e1014', borderRight: '1px solid #1b1e25'}} />
        <div style={{flex: 1, padding: '10px 10px'}}>
          {Array.from({length: lines}).map((_, line) => (
            <div
              key={line}
              style={{
                height: 5,
                marginBottom: 9,
                marginLeft: (line % 3) * 10,
                width: `${35 + random(`w-${index}-${line}`) * 50}%`,
                background: '#262a33',
                borderRadius: 3,
              }}
            />
          ))}
        </div>
        <div style={{width: 86, borderLeft: '1px solid #1b1e25', padding: '8px 7px', background: '#0f1115'}}>
          {isAgent
            ? [0, 1, 2, 3, 4].map((bubble) => {
                const grow = interpolate(frame, [agentStart + bubble * 9, agentStart + bubble * 9 + 14], [0, 1], clamp);
                return (
                  <div
                    key={bubble}
                    style={{
                      height: 5,
                      marginBottom: 8,
                      width: `${grow * (45 + random(`b-${index}-${bubble}`) * 50)}%`,
                      background: bubble === 0 ? '#3a3f4a' : '#2a2e37',
                      borderRadius: 3,
                    }}
                  />
                );
              })
            : null}
          {isAgent ? (
            <div
              style={{
                width: 6,
                height: 6,
                borderRadius: 3,
                marginTop: 6,
                background: palette.fg1,
                opacity: frame > agentStart ? 0.35 + 0.65 * Math.abs(Math.sin((frame - agentStart) / 9)) : 0,
              }}
            />
          ) : null}
        </div>
      </div>
    </div>
  );
};

const Counter: React.FC<{label: string; value: number; flashAt: number}> = ({label, value, flashAt}) => {
  const frame = useCurrentFrame();
  const flash = interpolate(frame, [flashAt, flashAt + 12], [1, 0], clamp);
  return (
    <div style={{display: 'flex', alignItems: 'baseline', gap: 14}}>
      <span style={{fontFamily: fonts.display, fontWeight: 800, fontSize: 64, color: palette.white, fontVariantNumeric: 'tabular-nums', textShadow: `0 0 ${24 * flash}px rgba(255,255,255,${0.6 * flash})`}}>
        {value}
      </span>
      <span style={{fontFamily: fonts.japanese, fontSize: 26, color: palette.fg1}}>{label}</span>
    </div>
  );
};

export const Lost: React.FC<{language: Language}> = ({language}) => {
  const frame = useCurrentFrame();
  const text = copy[language].lost;
  const promptText = language === 'en' ? typing.en : typing.ja;

  const windows = Math.min(20, Math.floor(frame / STEP) + 1);
  const agentTimes = AGENT_WINDOWS.map((_, order) => seconds(2.5) + order * 12);
  const agents = agentTimes.filter((time) => frame >= time).length;
  const branchTimes = [0, 1, 2].map((order) => seconds(3.5) + order * 15);
  const branches = branchTimes.filter((time) => frame >= time).length;

  const drift = interpolate(frame, [0, seconds(8)], [0.97, 1.05]);
  const shake = frame >= ERROR_AT && frame < ERROR_AT + 12 ? Math.sin(frame * 2.7) * (12 - (frame - ERROR_AT)) * 1.1 : 0;
  const dim = interpolate(frame, [seconds(4.4), seconds(4.7)], [1, 0.38], clamp) * interpolate(frame, [seconds(6.3), seconds(6.5)], [1, 0.85], clamp);

  // 送信: 入力欄の文字が「違う窓」へ吸い込まれていく
  const composerIn = interpolate(frame, [seconds(4.45), seconds(4.75)], [0, 1], {...clamp, easing: Easing.out(Easing.cubic)});
  const flight = interpolate(frame, [SEND_AT, SEND_AT + 14], [0, 1], {...clamp, easing: Easing.in(Easing.cubic)});
  const target = windowPosition(WRONG_WINDOW);
  const targetX = target.left + WINDOW_WIDTH - 43 - 960;
  const targetY = target.top + 60 - 900;

  const hudOut = interpolate(frame, [seconds(4.4), seconds(4.7)], [1, 0], clamp);

  return (
    <AbsoluteFill style={{background: palette.ink}}>
      <AbsoluteFill style={{transform: `scale(${drift}) translateX(${shake}px)`, filter: `brightness(${dim})`}}>
        {Array.from({length: 20}).map((_, index) => (
          <GrayWindow key={index} index={index} frame={frame} />
        ))}
      </AbsoluteFill>

      <div style={{position: 'absolute', left: (1920 - GRID_WIDTH) / 2, top: 78, display: 'flex', gap: 64, opacity: hudOut}}>
        <Counter label={text.windows} value={windows} flashAt={Math.round((windows - 1) * STEP)} />
        <Counter label={text.agents} value={agents} flashAt={agents > 0 ? agentTimes[agents - 1] : -100} />
        <Counter label={text.branches} value={branches} flashAt={branches > 0 ? branchTimes[branches - 1] : -100} />
      </div>

      {frame >= seconds(4.45) && frame < SEND_AT + 16 ? (
        <div
          style={{
            position: 'absolute',
            left: 960 - 460,
            top: 900 - 64,
            width: 920,
            height: 128,
            borderRadius: 16,
            background: '#15171c',
            border: '1px solid #2c3039',
            boxShadow: '0 30px 80px rgba(0,0,0,0.7)',
            opacity: composerIn * (1 - flight),
            transform: `translate(${targetX * flight}px, ${targetY * flight + (1 - composerIn) * 40}px) scale(${1 - flight * 0.85})`,
            padding: '22px 28px',
            boxSizing: 'border-box',
          }}
        >
          <div style={{fontFamily: fonts.japanese, fontSize: 18, color: palette.fg2, marginBottom: 10}}>
            {language === 'en' ? 'to: ?' : '送り先: ?'}
          </div>
          <Typewriter text={promptText} at={TYPE_AT} interval={TYPE_INTERVAL} caret style={{fontFamily: fonts.japanese, fontSize: 40, color: palette.white}} />
        </div>
      ) : null}

      <AbsoluteFill style={{alignItems: 'center', justifyContent: 'center', pointerEvents: 'none'}}>
        <div style={{fontFamily: fonts.japanese, fontWeight: 700, fontSize: 92, color: palette.white, textShadow: '0 8px 40px rgba(0,0,0,0.9)', textAlign: 'center'}}>
          <Rise text={text.wrong} at={seconds(6.45)} out={seconds(7.7)} />
        </div>
      </AbsoluteFill>
    </AbsoluteFill>
  );
};

