import React from 'react';
import {Composition} from 'remotion';
import {loadFonts} from './fonts';
import {Promo, type PromoProps} from './Promo';
import {DURATION, FPS} from './timeline';

loadFonts();

export const RemotionRoot: React.FC = () => (
  <Composition
    id="NecoderPromo"
    component={Promo}
    durationInFrames={DURATION}
    fps={FPS}
    width={1920}
    height={1080}
    defaultProps={{language: 'ja'} satisfies PromoProps}
  />
);
