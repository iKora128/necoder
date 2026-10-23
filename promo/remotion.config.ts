// Remotion の書き出し設定。60fps・1080p・高めの画質（ロゴや細い色線を潰さない）。
import {Config} from '@remotion/cli/config';

Config.setEntryPoint('src/index.ts');
Config.setVideoImageFormat('jpeg');
Config.setJpegQuality(95);
Config.setCodec('h264');
Config.setCrf(14);
Config.setPixelFormat('yuv420p');
Config.setChromiumOpenGlRenderer('angle');
