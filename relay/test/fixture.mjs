// 隔離されたテスト用 GUI。実ユーザーの socket / 設定 / agent には接続しない。
import http from 'node:http';
import { Bridge } from '../host/bridge.mjs';
import webpush from 'web-push';
let online = true, counter = 0;
const detail = { id: 'thread-a', turn_id: 'turn-a', entries: [{ kind: 'agent', text: 'リモート接続のテストです。' }], permission: null, question: null };
const snapshot = { instance_id: 'fixture-a', projects: [{ id: 'project-a', name: 'necoder-test', branch: 'remote-pwa', threads: [
  { id: 'thread-a', name: 'PWA integration', agent: 'Codex', turn_id: 'turn-a', running: false, tokens_used: 1234 },
] }, { id: 'private-project', name: '共有しないプロジェクト', threads: [] }] };
const bridge = new Bridge({ origin: 'http://localhost:8791', provisionToken: 'local-test-only', name: 'Test Mac', vapid: webpush.generateVAPIDKeys() }, [], {
  persist: async () => {}, persistReceipts: async () => {},
  ipc: async (method, params) => {
    if (!online) throw new Error('gui_offline');
    if (method === 'remote_snapshot') return structuredClone(snapshot);
    if (method === 'remote_thread') return structuredClone(detail);
    if (method === 'remote_get_diff') return { diff: 'diff --git a/example.rs b/example.rs\n+remote ready', tracked_only: true };
    if (params.turn_id !== detail.turn_id) throw new Error('stale_turn');
    if (method === 'remote_send_message') { detail.entries.push({ kind: 'user', text: params.message }); counter++; }
    else if (method === 'remote_permission_response') { if (params.permission_id !== detail.permission?.id) throw new Error('stale_permission'); detail.permission = null; }
    else if (method === 'remote_question_response') { detail.question = null; }
    else if (method === 'remote_interrupt') snapshot.projects[0].threads[0].running = false;
    else throw new Error('method_not_implemented');
    return { accepted: true };
  },
});
await bridge.poll();
const timer = setInterval(() => bridge.poll().catch(console.error), 100);
const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, 'http://127.0.0.1:8792');
    let result = { ok: true };
    if (url.pathname === '/pair') result = await bridge.pair('Browser test', ['project-a']);
    else if (url.pathname === '/offline') { online = false; await bridge.poll(); }
    else if (url.pathname === '/online') { online = true; await bridge.poll(); }
    else if (url.pathname === '/disconnect') { for (const state of bridge.connections.values()) state.socket.terminate(); }
    else if (url.pathname === '/permission') {
      detail.permission = { id: 'permission-a', title: 'Bash: echo test', complete: true, raw_input: '{"command":"echo test"}', diffs: [],
        options: [{ id: 'permission-a:0', kind: 'allow' }, { id: 'permission-a:1', kind: 'reject' }] };
    } else if (url.pathname === '/revoke') result = await bridge.revoke(url.searchParams.get('room'));
    else if (url.pathname === '/count') result = { count: counter };
    res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(result));
  } catch (error) { res.writeHead(500); res.end(error.message); }
});
server.listen(8792, '127.0.0.1');
function stop() { clearInterval(timer); bridge.stop(); server.close(); }
process.once('SIGTERM', stop); process.once('SIGINT', stop);
