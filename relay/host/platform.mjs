import path from 'node:path';
import { createHash } from 'node:crypto';

// GUI 側 paths::runtime_socket と同じ規則。OS を引数にし、Mac 上でも Windows を検証する。
export function locations(platform, env, home) {
  const paths = platform === 'win32' ? path.win32 : path.posix;
  const stateDir = env.NECODER_REMOTE_HOME || (platform === 'win32'
    ? paths.join(env.LOCALAPPDATA || paths.join(home, 'AppData', 'Local'), 'necoder', 'remote')
    : paths.join(home, '.necoder', 'remote'));
  const user = [...(env.USERNAME || 'default')].map(char => /^[A-Za-z0-9_-]$/.test(char) ? char : '_').join('');
  const guiSocket = env.NECODER_GUI_SOCK || (platform === 'win32' ? `\\\\.\\pipe\\necoder-gui-${user}`
    : platform === 'linux' && env.XDG_RUNTIME_DIR ? paths.join(env.XDG_RUNTIME_DIR, 'necoder', 'gui.sock')
    : paths.join(home, '.necoder', 'gui.sock'));
  const adminSocket = platform === 'win32'
    ? `\\\\.\\pipe\\necoder-remote-${createHash('sha256').update(stateDir.toLowerCase()).digest('hex').slice(0, 24)}`
    : paths.join(stateDir, 'host.sock');
  return { stateDir, guiSocket, adminSocket };
}
