// 同梱フォントを読み込む（すべて OFL-1.1。Archivo は LP と同じ書体の可変版）。
import {loadFont} from '@remotion/fonts';
import {staticFile} from 'remotion';

export const loadFonts = () =>
  Promise.all([
    loadFont({family: 'Archivo', url: staticFile('fonts/Archivo-Variable.ttf'), weight: '100 900', stretch: '62% 125%'}),
    loadFont({family: 'IBM Plex Sans JP', url: staticFile('fonts/IBMPlexSansJP-Regular.ttf'), weight: '400'}),
    loadFont({family: 'IBM Plex Sans JP', url: staticFile('fonts/IBMPlexSansJP-SemiBold.ttf'), weight: '600'}),
    loadFont({family: 'IBM Plex Sans JP', url: staticFile('fonts/IBMPlexSansJP-Bold.ttf'), weight: '700'}),
    loadFont({family: 'Guguru Sans Code', url: staticFile('fonts/GuguruSansCode-Regular.ttf'), weight: '400'}),
    loadFont({family: 'Guguru Sans Code', url: staticFile('fonts/GuguruSansCode-Bold.ttf'), weight: '700'}),
  ]);
