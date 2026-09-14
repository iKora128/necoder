import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';

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

/// **蓋のある mac か**（`AppleClamshellState` は蓋のある機種にだけ存在する）。
///
/// power assertion（`caffeinate`）は idle sleep しか止められず、**蓋を閉じるスリープは
/// 止まらない**。止められるのは clamshell 条件（AC + 外部ディスプレイ）を満たすか、
/// `sudo pmset -a disablesleep 1` を入れた場合だけ。前者は necoder からは作れず、後者は
/// root・システム全体・再起動を跨いで永続なので**勝手にやらない**。代わりに黙って
/// 落ちないよう「この機種は蓋を閉じると切れる」ことを申告する。
export function hasLid(platform = process.platform, run = null) {
  if (platform !== 'darwin') return false;
  try {
    const execute = run || ((file, args) => execFileSync(file, args, { timeout: 5000, encoding: 'utf8' }));
    return execute('/usr/sbin/ioreg', ['-r', '-k', 'AppleClamshellState']).includes('AppleClamshellState');
  } catch { return false; }
}
