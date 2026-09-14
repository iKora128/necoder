import { test } from 'node:test';
import assert from 'node:assert/strict';
import { hasLid, locations } from '../host/platform.mjs';
test('GUI と同じ Windows 名前付きパイプ・非 roaming 保存先', () => {
  const result = locations('win32', { USERNAME: 'CORP\\a.b', LOCALAPPDATA: 'C:\\Users\\test\\AppData\\Local' }, 'C:\\Users\\test');
  assert.equal(result.guiSocket, '\\\\.\\pipe\\necoder-gui-CORP_a_b');
  assert.equal(result.stateDir, 'C:\\Users\\test\\AppData\\Local\\necoder\\remote');
  assert.match(result.adminSocket, /^\\\\\.\\pipe\\necoder-remote-[a-f0-9]{24}$/);
});
test('Mac の既存パスと Linux XDG、明示 override', () => {
  assert.equal(locations('darwin', {}, '/Users/test').guiSocket, '/Users/test/.necoder/gui.sock');
  assert.equal(locations('linux', { XDG_RUNTIME_DIR: '/run/user/1000' }, '/home/test').guiSocket, '/run/user/1000/necoder/gui.sock');
  assert.equal(locations('win32', { NECODER_GUI_SOCK: 'explicit' }, 'C:\\Users\\test').guiSocket, 'explicit');
});
test('蓋のある mac だけを蓋スリープの対象と判定する', () => {
  const withLid = '  | "AppleClamshellState" = No\n';
  assert.equal(hasLid('darwin', () => withLid), true);
  assert.equal(hasLid('darwin', () => '  | "IOPlatformUUID" = "x"\n'), false);
  // 蓋の概念が無い OS では ioreg を呼ばない。
  assert.equal(hasLid('win32', () => { throw new Error('should not run'); }), false);
  assert.equal(hasLid('linux', () => { throw new Error('should not run'); }), false);
  // ioreg が無い/失敗する環境で pair を落とさない。
  assert.equal(hasLid('darwin', () => { throw new Error('ENOENT'); }), false);
});
