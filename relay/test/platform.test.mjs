import { test } from 'node:test';
import assert from 'node:assert/strict';
import { locations } from '../host/platform.mjs';
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
