// 本編。場面を小節で並べ、上から質感（グレイン・周辺減光）と音を重ねる。
import React from 'react';
import {AbsoluteFill, Audio, Sequence, staticFile} from 'remotion';
import {Grain, Vignette} from './components/Overlay';
import type {Language} from './copy';
import {Agents} from './scenes/Agents';
import {Built} from './scenes/Built';
import {Color} from './scenes/Color';
import {Logo} from './scenes/Logo';
import {Lost} from './scenes/Lost';
import {Outro} from './scenes/Outro';
import {Power} from './scenes/Power';
import {Touches} from './scenes/Touches';
import {palette} from './theme';
import {scenes} from './timeline';

export type PromoProps = {language: Language};

export const Promo: React.FC<PromoProps> = ({language}) => (
  <AbsoluteFill style={{background: palette.ink}}>
    <Sequence from={scenes.lost.from} durationInFrames={scenes.lost.length} name="迷子">
      <Lost language={language} />
    </Sequence>
    <Sequence from={scenes.built.from} durationInFrames={scenes.built.length} name="だから作った">
      <Built language={language} />
    </Sequence>
    <Sequence from={scenes.logo.from} durationInFrames={scenes.logo.length} name="ロゴ">
      <Logo language={language} />
    </Sequence>
    <Sequence from={scenes.color.from} durationInFrames={scenes.color.length} name="色">
      <Color language={language} />
    </Sequence>
    <Sequence from={scenes.agents.from} durationInFrames={scenes.agents.length} name="AI">
      <Agents language={language} />
    </Sequence>
    <Sequence from={scenes.touches.from} durationInFrames={scenes.touches.length} name="かゆいところ">
      <Touches language={language} />
    </Sequence>
    <Sequence from={scenes.power.from} durationInFrames={scenes.power.length} name="スマホ・SSH・性能">
      <Power language={language} />
    </Sequence>
    <Sequence from={scenes.outro.from} durationInFrames={scenes.outro.length} name="締め">
      <Outro language={language} />
    </Sequence>
    <Vignette />
    <Grain />
    <Audio src={staticFile(`soundtrack-${language}.wav`)} />
  </AbsoluteFill>
);
