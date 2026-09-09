import { mkdir, readFile, writeFile, rename, chmod } from 'node:fs/promises';
import { homedir } from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { locations } from './platform.mjs';
export const { stateDir, adminSocket, guiSocket } = locations(process.platform, process.env, homedir());
let secured;
export async function secureDirectory() {
  if (!secured) secured = (async () => {
    await mkdir(stateDir, { recursive: true, mode: 0o700 });
    if (process.platform !== 'win32') { await chmod(stateDir, 0o700); return; }
    // chmod は Windows の ACL を制限しない。作成ユーザーと SYSTEM だけへ完全制御を付与。
    const script = "$ErrorActionPreference='Stop'; $acl = New-Object System.Security.AccessControl.DirectorySecurity; "
      + "$acl.SetAccessRuleProtection($true,$false); $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; "
      + "$acl.SetOwner($sid); foreach ($id in @($sid, [System.Security.Principal.SecurityIdentifier]'S-1-5-18')) { "
      + "$rule=New-Object System.Security.AccessControl.FileSystemAccessRule($id,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); $acl.AddAccessRule($rule) }; "
      + "Set-Acl -LiteralPath $env:NECODER_ACL_TARGET -AclObject $acl";
    await promisify(execFile)('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], {
      windowsHide: true, env: { ...process.env, NECODER_ACL_TARGET: path.resolve(stateDir) }, timeout: 15000,
    });
  })().catch(error => { secured = null; throw error; });
  return secured;
}
export async function readState(name, fallback = null) {
  try { return JSON.parse(await readFile(path.join(stateDir, name + '.json'), 'utf8')); }
  catch (error) { if (error.code === 'ENOENT') return fallback; throw error; }
}
let writes = Promise.resolve();
export function writeState(name, value) {
  const serialized = JSON.stringify(value);
  const operation = writes.then(async () => {
    await secureDirectory();
    const destination = path.join(stateDir, name + '.json');
    const temporary = destination + '.tmp';
    await writeFile(temporary, serialized, { mode: 0o600 });
    await chmod(temporary, 0o600);
    await rename(temporary, destination);
  });
  writes = operation.catch(() => {});
  return operation;
}
