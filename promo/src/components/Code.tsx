// コードの表示。トークン分けは Rust 向けの素朴なもの（色はアプリの syntax トークン）。
import React from 'react';
import {fonts, palette, syntax} from '../theme';

type TokenKind = keyof typeof syntax;
type Token = {text: string; kind: TokenKind};

const KEYWORDS = new Set([
  'as', 'async', 'await', 'const', 'crate', 'dyn', 'else', 'enum', 'false', 'fn', 'for', 'if', 'impl', 'in', 'let',
  'loop', 'match', 'mod', 'move', 'mut', 'pub', 'ref', 'return', 'self', 'Self', 'static', 'struct', 'super', 'trait',
  'true', 'type', 'unsafe', 'use', 'where', 'while',
]);

export const tokenize = (line: string): Token[] => {
  const tokens: Token[] = [];
  let rest = line;
  const take = (text: string, kind: TokenKind) => {
    tokens.push({text, kind});
    rest = rest.slice(text.length);
  };
  while (rest.length > 0) {
    const comment = rest.match(/^\/\/.*/);
    if (comment) {
      take(comment[0], 'comment');
      continue;
    }
    const text = rest.match(/^"(?:[^"\\]|\\.)*"?/);
    if (text) {
      take(text[0], 'string');
      continue;
    }
    const attribute = rest.match(/^#\[[^\]]*\]/);
    if (attribute) {
      take(attribute[0], 'macro');
      continue;
    }
    const number = rest.match(/^\d[\d_]*(?:\.\d*)?/);
    if (number) {
      take(number[0], 'number');
      continue;
    }
    const word = rest.match(/^[A-Za-z_][A-Za-z0-9_]*/);
    if (word) {
      const after = rest.slice(word[0].length);
      let kind: TokenKind = 'plain';
      if (KEYWORDS.has(word[0])) {
        kind = 'keyword';
      } else if (after.startsWith('!')) {
        kind = 'macro';
      } else if (after.startsWith('(') || after.startsWith('::<')) {
        kind = 'function';
      } else if (/^[A-Z]/.test(word[0])) {
        kind = 'type';
      }
      take(word[0], kind);
      continue;
    }
    const space = rest.match(/^\s+/);
    if (space) {
      take(space[0], 'plain');
      continue;
    }
    take(rest[0], 'punctuation');
  }
  return tokens;
};

export const CodeLine: React.FC<{line: string}> = ({line}) => (
  <>
    {tokenize(line).map((token, index) => (
      <span key={index} style={{color: syntax[token.kind], fontStyle: token.kind === 'comment' ? 'italic' : 'normal'}}>
        {token.text}
      </span>
    ))}
  </>
);

export const CodeBlock: React.FC<{
  lines: string[];
  firstLine?: number;
  fontSize: number;
  lineHeight?: number;
  reveal?: (index: number) => number;
  highlight?: (index: number) => number;
  highlightColor?: string;
  style?: React.CSSProperties;
}> = ({lines, firstLine = 1, fontSize, lineHeight = 1.6, reveal, highlight, highlightColor = palette.white, style}) => (
  <div style={{fontFamily: fonts.mono, fontSize, lineHeight, color: palette.fg0, ...style}}>
    {lines.map((line, index) => {
      const visible = reveal ? reveal(index) : 1;
      const strength = highlight ? highlight(index) : 0;
      return (
        <div
          key={index}
          style={{
            display: 'flex',
            opacity: visible,
            transform: `translateX(${(1 - visible) * 24}px)`,
            background: strength > 0 ? `rgba(255,255,255,${0.05 * strength})` : 'transparent',
            boxShadow: strength > 0 ? `inset ${3 * strength}px 0 0 ${highlightColor}` : 'none',
          }}
        >
          <span style={{width: fontSize * 3, flex: 'none', textAlign: 'right', paddingRight: fontSize, color: palette.fg2}}>
            {firstLine + index}
          </span>
          <span style={{whiteSpace: 'pre'}}>
            <CodeLine line={line} />
          </span>
        </div>
      );
    })}
  </div>
);
