// PWA の見た目を iPhone サイズで撮る
// （`node test/preview.mjs [--data <dir>] [--out <dir>] [--thread <名前の一部>,…] [--pages N] [--sheets] [--locale en-US]`）。
// 隔離: リレーは wrangler dev のテスト設定（:8791）、GUI は偽物（データは JSON）。実ユーザーの
// socket / 設定 / 端末には触らない。`--data` には remote_snapshot の結果（snapshot.json）と
// thread_id → remote_thread の結果（details.json）を置く。省略時は test/preview-fixture.json。
// **実データの置き場は relay/ の外にする**（build が test/ ごと public/source.tar.gz へ固めて公開する）。
import { spawn } from 'node:child_process';
import { mkdir, readFile } from 'node:fs/promises';
import { webkit, devices } from '@playwright/test';
import webpush from 'web-push';
import { Bridge } from '../host/bridge.mjs';

// `--名前 値` / 値の無い `--名前` は true（例: `--sheets` = 設定と新規スレッドのシートも撮る）。
const options = Object.fromEntries(process.argv.slice(2).reduce((pairs, value, index, all) => value.startsWith('--')
  ? [...pairs, [value.slice(2), all[index + 1] && !all[index + 1].startsWith('--') ? all[index + 1] : true]] : pairs, []));
const out = options.out || 'test-results/preview';
await mkdir(out, { recursive: true });
let snapshot, details, later;
if (options.data) {
  snapshot = JSON.parse(await readFile(`${options.data}/snapshot.json`, 'utf8'));
  details = JSON.parse(await readFile(`${options.data}/details.json`, 'utf8'));
} else {
  ({ snapshot, details, later } = JSON.parse(await readFile(new URL('./preview-fixture.json', import.meta.url), 'utf8')));
}

const relay = spawn('npx', ['wrangler', 'dev', '--config', 'wrangler.test.jsonc', '--port', '8791', '--ip', '127.0.0.1'],
  { stdio: ['ignore', 'ignore', 'inherit'], detached: true });
const stopRelay = () => { try { process.kill(-relay.pid, 'SIGTERM'); } catch { /* 既に終了 */ } };
process.once('exit', stopRelay);
for (let attempt = 0; ; attempt++) {
  try { if ((await fetch('http://localhost:8791/api/health')).ok) break; } catch { /* 起動待ち */ }
  if (attempt > 120) throw new Error('wrangler dev が起動しない');
  await new Promise(resolve => setTimeout(resolve, 500));
}

const bridge = new Bridge({ origin: 'http://localhost:8791', name: 'Preview Mac', vapid: webpush.generateVAPIDKeys() }, [], {
  persist: async () => {}, persistReceipts: async () => {},
  ipc: async (method, params) => {
    if (method === 'remote_snapshot') return structuredClone(snapshot);
    if (method === 'remote_thread') return structuredClone(details[params.thread_id] ?? { error: 'thread_not_found' });
    if (method === 'remote_get_diff') return { diff: 'diff --git a/src/main.rs b/src/main.rs\n-old\n+new', tracked_only: true };
    return { accepted: true };
  },
});
await bridge.poll();
const poller = setInterval(() => bridge.poll().catch(console.error), 200);

const browser = await webkit.launch();
try {
  const context = await browser.newContext({ ...devices['iPhone 13'], locale: options.locale || 'ja-JP', colorScheme: 'dark' });
  const page = await context.newPage();
  page.on('pageerror', error => console.error('pageerror:', error.message));
  const shot = async name => { await page.screenshot({ path: `${out}/${name}.png` }); console.log(`${out}/${name}.png`); };
  await page.goto('http://localhost:8791/');
  await shot('0-welcome');
  const pairing = await bridge.pair('Preview', snapshot.projects.map(project => project.id));
  // 同じ文書への fragment 遷移は再読込されない（ペアリングが走らない）ので一度離れる。
  await page.goto('about:blank');
  await page.goto(pairing.url);
  await page.waitForFunction(() => document.body.dataset.status === 'online', null, { timeout: 20_000 }).catch(async error => {
    await shot('x-timeout');
    console.error('status:', await page.locator('#status').textContent(), 'notice:', await page.locator('#notice').textContent());
    throw error;
  });
  await page.waitForTimeout(800);
  await shot('1-home');
  await page.screenshot({ path: `${out}/1-home-full.png`, fullPage: true });
  // fixture の `later`: 撮った後にターンを進めて「新着」の出方を見る。
  if (later?.length) {
    for (const change of later) for (const project of snapshot.projects) for (const thread of project.threads)
      if (thread.id === change.thread) thread.turn_id = change.turn_id;
    await page.waitForTimeout(800);
    await shot('1-home-later');
  }
  // `--thread 名前の一部,名前の一部`: 一覧から開いて撮る（`--pages N` で最新から上へ N 画面）。
  for (const [index, needle] of String(options.thread ?? '').split(',').filter(Boolean).entries()) {
    await page.locator('[data-thread-name]').filter({ hasText: needle }).first().click();
    await page.waitForTimeout(1200);
    await shot(`2-thread-${index}`);
    // `--expand`: 作業の欄を開いた姿も撮る（最後の箱を開いて、その中の最初の 1 件も開く）。
    if (options.expand && await page.locator('button.work-head').count()) {
      await page.locator('button.work-head').last().click();
      await page.locator('.work-list button.step').first().click().catch(() => {});
      await page.waitForTimeout(200);
      await page.locator('.workbox').last().scrollIntoViewIfNeeded();
      await shot(`2-thread-${index}-open`);
    }
    for (let up = 1; up < Number(options.pages || 1); up++) {
      await page.evaluate(() => window.scrollBy(0, -window.innerHeight * 0.85));
      await page.waitForTimeout(150);
      await shot(`2-thread-${index}-up${up}`);
    }
    await page.locator('#back').click();
    await page.waitForTimeout(300);
  }
  if (options.sheets) {
    await page.locator('#open-settings').click(); await page.waitForTimeout(300); await shot('3-settings');
    await page.locator('#close-settings').click();
    await page.locator('.project-head .add').first().click(); await page.waitForTimeout(300); await shot('3-new-thread');
    await page.locator('#close-agent').click();
  }
} finally {
  await browser.close();
  clearInterval(poller); bridge.stop(); stopRelay();
}
process.exit(0);
