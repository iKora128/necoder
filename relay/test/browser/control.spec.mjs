import { test, expect } from '@playwright/test';
import QRCode from 'qrcode';

test('PWA: ペアリング・限定共有・送信・承認・再接続・失効', async ({ page, request }, testInfo) => {
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const count = (await (await request.get('http://127.0.0.1:8792/count')).json()).count;
  const pairing = await (await request.get('http://127.0.0.1:8792/pair')).json();
  const room = new URLSearchParams(new URL(pairing.url).hash.slice(1)).get('room');
  await page.goto(pairing.url);
  await expect(page.locator('#status')).toHaveText('接続済み');
  await expect(page.locator('#destination')).toContainText('PWA integration');
  expect(new URL(page.url()).hash).toBe('');
  await expect(page.locator('#projects option')).toHaveCount(1);
  // fixture（transcript と送信の数）は chromium → webkit の順で project をまたいで共有される。本文が同じだと
  // webkit の回では chromium が送った同じ文で toContainText が先に通り、数を見る時に自分の送信がまだ
  // 届いていないことがある。本文を project ごとに変え、数は届くまで待つ。
  const message = `スマホからのテスト指示（${testInfo.project.name}）`;
  await page.locator('#message').fill(message);
  await page.locator('#send').click();
  await expect(page.locator('#transcript')).toContainText(message);
  // 新しい順: 送ったばかりの発話が入力欄のすぐ下（先頭）に来る。
  await expect(page.locator('#transcript .entry').first()).toContainText(message);
  await expect.poll(async () => (await (await request.get('http://127.0.0.1:8792/count')).json()).count).toBe(count + 1);
  await page.locator('#diff').click();
  await expect(page.locator('#diff-text')).toContainText('+remote ready');
  await page.locator('#close-diff').click();
  await request.get('http://127.0.0.1:8792/permission');
  await expect(page.getByRole('button', { name: '今回だけ許可' })).toBeDisabled();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: '今回だけ許可' }).click();
  await expect(page.locator('#permission')).toBeEmpty();
  await page.reload();
  await expect(page.locator('#status')).toHaveText('接続済み');
  await request.get('http://127.0.0.1:8792/offline');
  await expect(page.locator('#status')).toHaveText('necoderが起動していません');
  await expect(page.locator('#send')).toBeDisabled();
  await request.get('http://127.0.0.1:8792/online');
  await expect(page.locator('#status')).toHaveText('接続済み');
  await request.get('http://127.0.0.1:8792/disconnect');
  await expect(page.locator('#status')).toHaveText('PCに接続できません');
  await expect(page.locator('#status')).toHaveText('接続済み', { timeout: 15000 });
  await expect(page.locator('#transcript')).toContainText(message);
  await page.screenshot({ path: `test-results/pwa-mobile-${testInfo.project.name}.png`, fullPage: true });
  await request.get(`http://127.0.0.1:8792/revoke?room=${room}`);
  await expect(page.locator('#send')).toBeDisabled();
  expect(errors).toEqual([]);
});

test('部屋作成はアカウント不要・壊れた資格情報と異なる Origin を拒否、PWA 配信ヘッダー', async ({ request, page }) => {
  const room = 'a'.repeat(43);
  // アカウント不要の公開リレー: 秘密を持たない相手でも部屋は作れる（濫用対策はレート制限と PoW）。
  expect((await request.post(`/api/rooms/${room}`, { data: {} })).status()).toBe(400);
  const credentials = { host: 'h'.repeat(43), phone: 'p'.repeat(43) };
  // DO の状態は project（chromium/webkit）を跨いで残るので、部屋は毎回別の id で作る。
  const fresh = Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString('base64url');
  expect((await request.post(`/api/rooms/${fresh}`, { data: credentials })).status()).toBe(201);
  // 同じ部屋の二重作成は拒否（room id は 256bit 乱数なので実運用では起きない）。
  expect((await request.post(`/api/rooms/${fresh}`, { data: credentials })).status()).toBe(409);
  expect((await request.get('/api/health')).ok()).toBe(true);
  expect((await (await request.get('/api/health')).json()).difficulty).toBe(0);
  expect((await request.get('/api/health', { headers: { Origin: 'https://evil.example' } })).status()).toBe(403);
  const response = await page.goto('/');
  expect(response.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
  await expect(page.getByRole('heading', { name: 'PCの作業を、手元から。' })).toBeVisible();
  await page.screenshot({ path: 'test-results/pwa-welcome.png', fullPage: true });
});

/// 偽カメラを `getUserMedia` に差し込む（`qr` = QR を写す / `denied` = 拒否）。
/// **`page.goto` の後に呼ぶこと**（アプリがカメラを触るのはボタンを押した後なので、文書ができてからで足りる）。
///
/// **`MediaDevices.prototype` に載せる。`navigator.mediaDevices`（インスタンス）の own property にしない。**
/// WebKit は `MediaDevices` の JS ラッパーを GC で回収し、次のアクセスで作り直す（IDL に
/// `GenerateIsReachable` が無い ＝ JS 側から参照が切れれば回収対象。`Navigator` の方は
/// `GenerateIsReachable=ReachableFromDOMWindow` で window と同寿命）。インスタンスに載せた own property は
/// ラッパーごと消えるので、差し込み直後の `expectFakeCamera` は通るのに、ボタンを押した時には本物の
/// getUserMedia が呼ばれて permission 待ちのまま固まる。GC のタイミング次第で落ちる純粋なフレークで、
/// relay に 1 行の変更も無いまま CI の WebKit だけ 1 勝 4 敗だった（2026-09-14〜09-24。落ちるテストも
/// 回ごとに違った）。Chromium はラッパーを保持するので own property でも通ってしまい、手元では気づけない。
/// prototype は global object が握っていて回収されない（Playwright WebKit 26 / revision 2248 で、
/// GC 圧をかけてもインスタンス側は消え prototype 側は残ることを確認済み・2026-09-24）。
///
/// 2026-09-14 に `addInitScript` で効かなかったのもこれで説明がつく（init script は
/// `navigator.mediaDevices` に own property を載せていた）。「文書が確定する時に navigator を作り直す」
/// という当時の読みは誤診。
///
/// `navigator.mediaDevices` は secure context 以外や一部の WebView では生えない。その環境だけ
/// `navigator` 側に器を作る（`Navigator` のラッパーは window と同寿命なので own property でも残る）。
async function installFakeCamera(page, mode, image = null) {
  await page.evaluate(([mode, source]) => {
    const getUserMedia = mode === 'denied'
      ? async () => { throw new DOMException('denied', 'NotAllowedError'); }
      : async () => {
        const canvas = document.createElement('canvas');
        canvas.width = canvas.height = 512;
        const context = canvas.getContext('2d');
        const picture = new Image();
        await new Promise(resolve => { picture.onload = resolve; picture.src = source; });
        const draw = () => { context.drawImage(picture, 0, 0, 512, 512); requestAnimationFrame(draw); };
        draw();
        return canvas.captureStream(15);
      };
    getUserMedia.fakeCamera = true;
    if (typeof MediaDevices === 'function' && navigator.mediaDevices)
      Object.defineProperty(MediaDevices.prototype, 'getUserMedia', { configurable: true, writable: true, value: getUserMedia });
    else
      Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: { getUserMedia } });
  }, [mode, image]);
}

/// 偽カメラが**実際に呼ばれる**か。差し込んだつもりで本物が残っていると固まるだけなので、
/// 形（prototype に載っているか・インスタンスの own property に戻っていないか・印が付いているか）と
/// 呼び出し結果を両方見て、
/// 落ちた時は中身を報告する。
async function expectFakeCamera(page) {
  const report = await page.evaluate(async () => {
    const shape = {
      ownOnNavigator: !!Object.getOwnPropertyDescriptor(navigator, 'mediaDevices'),
      onPrototype: globalThis.MediaDevices?.prototype?.getUserMedia?.fakeCamera === true,
      // インスタンスの own property は WebKit の GC で消える側。true なら差し込み方が退行している。
      ownGetUserMedia: !!Object.getOwnPropertyDescriptor(navigator.mediaDevices ?? {}, 'getUserMedia'),
      marked: navigator.mediaDevices?.getUserMedia?.fakeCamera === true,
    };
    // 本物が残っていると permission 待ちで固まるので、待たずに hung として返す。
    const called = await Promise.race([
      navigator.mediaDevices.getUserMedia({ video: true }).then(stream => {
        for (const track of stream.getTracks()) track.stop();
        return 'resolved';
      }, error => error.name),
      new Promise(resolve => setTimeout(() => resolve('hung'), 3000)),
    ]);
    return { ...shape, called };
  });
  expect(report.marked, `偽カメラが差し込めていない: ${JSON.stringify(report)}`).toBe(true);
  expect(report.called, `偽カメラが呼ばれていない: ${JSON.stringify(report)}`).not.toBe('hung');
}

test('PWA 内で QR を読み取ってペアリングする', async ({ page, request, browserName }) => {
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const pairing = await (await request.get('http://127.0.0.1:8792/pair')).json();
  // 偽カメラ: Mac の画面に出る QR を canvas に描き、その captureStream を getUserMedia に返す。
  // 実際の読み取り経路（BarcodeDetector / jsQR）をそのまま通す。
  const image = await QRCode.toDataURL(pairing.url, { width: 512, margin: 2 });
  await page.goto('/');
  await installFakeCamera(page, 'qr', image);
  await expectFakeCamera(page);
  await page.locator('#scan').click();
  await expect(page.locator('#status')).toHaveText('接続済み', { timeout: 20000 });
  await expect(page.locator('#destination')).toContainText('PWA integration');
  // 読み取り後はカメラを必ず離す（ダイアログが閉じ、トラックが止まっている）。
  await expect(page.locator('#scan-dialog')).toBeHidden();
  expect(await page.evaluate(() => document.getElementById('scan-video').srcObject)).toBe(null);
  expect(errors).toEqual([]);
});

test('カメラが使えなければ行き止まりにせず貼り付けへ倒す', async ({ page }) => {
  // 落ちた時に「何も起きなかった」で終わらせない（CI だけで落ちた 2026-09-14 の反省）。
  const problems = [];
  page.on('pageerror', error => problems.push(`pageerror: ${error.message}`));
  page.on('console', message => { if (message.type() === 'error') problems.push(`console: ${message.text()}`); });
  await page.goto('/');
  await installFakeCamera(page, 'denied');
  await expectFakeCamera(page);
  await expect(page.locator('#pair-manual')).not.toHaveAttribute('open', '');
  await page.locator('#scan').click();
  // 押しても何も起きなかった時のために、押下後の状態を読む（ハンドラは居るか・
  // 偽カメラは残っているか・文言はどの言語か）。
  const state = await page.evaluate(() => ({
    handler: typeof document.getElementById('scan').onclick,
    stillFake: navigator.mediaDevices?.getUserMedia?.fakeCamera === true,
    error: document.getElementById('scan-error').textContent,
    manualOpen: document.getElementById('pair-manual').open,
    lang: document.documentElement.lang,
  }));
  const why = [problems.join(' / ') || '(ブラウザ側にエラーなし)', JSON.stringify(state)].join(' ');
  await expect(page.locator('#scan-error'), why).toContainText('カメラ');
  await expect(page.locator('#pair-manual')).toHaveAttribute('open', '');
  await expect(page.locator('#pair-url')).toBeVisible();
});
