// レールの 1 枠（30×30・角丸 8・色 14% の面 + 枠 2px。アクティブは枠が 100%）。rail_view.rs と同じ見た目。
import React from 'react';
import {fonts, palette} from '../theme';

const alpha = (hex: string, amount: number) => {
  const value = Math.round(Math.max(0, Math.min(1, amount)) * 255)
    .toString(16)
    .padStart(2, '0');
  return `${hex}${value}`;
};

export const RailItem: React.FC<{
  color: string;
  monogram: string;
  active: boolean;
  size: number;
  style?: React.CSSProperties;
  fill?: number;
  border?: number;
  radius?: number;
}> = ({color, monogram, active, size, style, fill = 0.14, border = 1, radius = 1}) => {
  const unit = size / 30;
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: 8 * unit * radius,
        background: alpha(color, fill),
        border: `${2 * unit}px solid ${alpha(color, active ? border : 0.35 * border)}`,
        boxShadow: active ? `0 0 0 ${2 * unit}px ${alpha(color, 0.28)}` : 'none',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        color: palette.fg0,
        fontFamily: fonts.japanese,
        fontWeight: 600,
        fontSize: 14 * unit,
        boxSizing: 'border-box',
        ...style,
      }}
    >
      {monogram}
    </div>
  );
};

export {alpha};
