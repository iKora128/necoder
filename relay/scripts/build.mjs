import { build } from 'esbuild';
import sharp from 'sharp';
import { mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = fileURLToPath(new URL('..', import.meta.url));
await mkdir(path.join(root, 'dist'), { recursive: true });
await build({ absWorkingDir: root, entryPoints: ['host/cli.mjs'], outfile: 'dist/host.mjs',
  bundle: true, platform: 'node', target: 'node22', format: 'esm',
  banner: { js: "import { createRequire } from 'node:module'; const require = createRequire(import.meta.url);" } });
for (const size of [192, 512]) await sharp(path.join(root, 'public/icon.svg')).resize(size, size).png().toFile(path.join(root, `public/icon-${size}.png`));
const lock = JSON.parse(await readFile(path.join(root, 'package-lock.json'), 'utf8'));
const packages = Object.entries(lock.packages).filter(([name, metadata]) => name && !metadata.dev).map(([name]) => path.join(root, name));
const notices = [];
for (const directory of packages) {
  const metadata = JSON.parse(await readFile(path.join(directory, 'package.json'), 'utf8'));
  const files = (await readdir(directory)).filter(name => /^(licen[sc]e|copying|notice)(\.|$)/i.test(name));
  notices.push(`${metadata.name}@${metadata.version} (${metadata.license || 'see package'})\n` +
    (await Promise.all(files.map(name => readFile(path.join(directory, name), 'utf8')))).join('\n'));
}
await writeFile(path.join(root, 'dist/THIRD_PARTY_LICENSES.txt'), notices.join('\n\n--------------------\n\n'));
// AGPL の対応ソース。秘密/開発状態/node_modules を含めず、公開するリレーとホストの全ソースを同梱。
execFileSync('tar', ['--exclude=public/source.tar.gz', '-czf', path.join(root, 'public/source.tar.gz'),
  '-C', root, 'src', 'host', 'public', 'scripts', 'test', 'package.json', 'package-lock.json', 'wrangler.jsonc', 'wrangler.test.jsonc', 'tsconfig.json', 'playwright.config.mjs', 'README.md',
  '-C', path.dirname(root), 'LICENSE'], { stdio: 'inherit' });
console.log('Built host, icons and corresponding source archive.');
