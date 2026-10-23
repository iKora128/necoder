import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transform } from 'esbuild';

const source = await readFile(new URL('../src/room_request.ts', import.meta.url), 'utf8');
const { code } = await transform(source, { loader: 'ts', format: 'esm' });
const { readRoomCredentials, MAX_ROOM_BODY } = await import('data:text/javascript;base64,' + Buffer.from(code).toString('base64'));
const credentials = { host: 'h'.repeat(43), phone: 'p'.repeat(43) };
const request = body => new Request('http://localhost/rooms', { method: 'POST', body });

test('部屋作成の本文を byte 単位で制限し、壊れた JSON と構造を拒否する', async () => {
  assert.deepEqual(await readRoomCredentials(request(JSON.stringify(credentials))), credentials);
  assert.deepEqual(await readRoomCredentials(request(JSON.stringify(credentials).padEnd(MAX_ROOM_BODY))), credentials);
  for (const body of ['null', '[]', '1', '"text"', '{}', '{"host":null}', JSON.stringify({ ...credentials, phone: credentials.host })]) {
    const result = await readRoomCredentials(request(body));
    assert.equal(result.status, 400, body);
  }
  for (const body of [' '.repeat(MAX_ROOM_BODY + 1), 'あ'.repeat(800)])
    assert.equal((await readRoomCredentials(request(body))).status, 413);
  assert.equal((await readRoomCredentials(request('{broken'))).status, 400);
});

test('Content-Length のないストリームも上限で打ち切り、それ以降を読み込まない', async () => {
  let pulled = 0, cancelled = false;
  const body = new ReadableStream({
    pull(controller) { pulled++; controller.enqueue(new Uint8Array(512)); },
    cancel() { cancelled = true; },
  });
  const streamed = new Request('http://localhost/rooms', { method: 'POST', body, duplex: 'half' });
  assert.equal(streamed.headers.has('Content-Length'), false);
  assert.equal((await readRoomCredentials(streamed)).status, 413);
  assert.equal(cancelled, true);
  assert.ok(pulled <= 6, `読み込みが止まっていない: ${pulled}`);
});
