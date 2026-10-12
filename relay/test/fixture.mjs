// 隔離されたテスト用 GUI。実ユーザーの socket / 設定 / agent には接続しない。
import http from 'node:http';
import { Bridge } from '../host/bridge.mjs';
import webpush from 'web-push';
let online = true, counter = 0;
const detail = { id: 'thread-a', turn_id: 'turn-a', entries: [{ kind: 'agent', text: 'リモート接続のテストです。' }], permission: null, question: null };
const list = pairs => pairs.map(([id, label]) => ({ id, label }));
// 新しいスレッドのシートの中身（GUI の remote.rs と同じ形）。Claude Code は在庫あり・Codex はまだ起動していない。
const newThread = { agents: [
  { name: 'Claude Code', default: true,
    model: { current: 'default', label: 'Default (recommended)', choices: list([['default', 'Default (recommended)'], ['opus', 'Opus 4.8'], ['sonnet', 'Sonnet 5']]) },
    effort: { current: 'default', label: 'Default', choices: list([['default', 'Default'], ['low', 'Low'], ['high', 'High']]) },
    mode: { current: 'default', label: 'Default', choices: [{ id: 'default', label: 'Default', allowed: true }, { id: 'plan', label: 'Plan Mode', allowed: true },
      { id: 'bypassPermissions', label: 'Bypass Permissions', allowed: false }] } },
  { name: 'Codex', default: false, model: { current: 'gpt-5.6-sol', label: 'gpt-5.6-sol', choices: [] },
    effort: { current: null, label: null, choices: [] }, mode: { current: null, label: null, choices: [] } },
] };
const snapshot = { instance_id: 'fixture-a', projects: [{ id: 'project-a', name: 'necoder-test', branch: 'remote-pwa', new_thread: newThread, threads: [
  { id: 'thread-a', name: 'PWA integration', agent: 'Codex', turn_id: 'turn-a', running: false, tokens_used: 1234 },
] }, { id: 'private-project', name: '共有しないプロジェクト', threads: [] }] };
const details = { 'thread-a': detail };
let created = 0;
const last = {}; // 最後に受けた new_thread / set_setting の引数（/last で返す）
const bridge = new Bridge({ origin: 'http://localhost:8791', name: 'Test Mac', vapid: webpush.generateVAPIDKeys() }, [], {
  persist: async () => {}, persistReceipts: async () => {},
  ipc: async (method, params) => {
    if (!online) throw new Error('gui_offline');
    if (method === 'remote_snapshot') return structuredClone(snapshot);
    if (method === 'remote_thread') return structuredClone(details[params.thread_id] ?? detail);
    if (method === 'remote_get_diff') return { diff: 'diff --git a/example.rs b/example.rs\n+remote ready', tracked_only: true };
    if (method === 'remote_new_thread') {
      const agent = newThread.agents.find(candidate => candidate.name === params.agent);
      if (!agent) throw new Error('unknown_agent');
      last.new_thread = { agent: params.agent, model: params.model, effort: params.effort, mode: params.mode };
      const id = `thread-new-${++created}`, turn = `turn-${id}`;
      const pick = (setting, value) => ({ ...setting, current: value ?? setting.current });
      const settings = { agent: { current: agent.name, fixed: false, choices: list(newThread.agents.map(candidate => [candidate.name, candidate.name])) },
        model: pick(agent.model, params.model), effort: pick(agent.effort, params.effort), mode: pick(agent.mode, params.mode) };
      const label = setting => setting.choices.find(choice => choice.id === setting.current)?.label ?? setting.current;
      snapshot.projects[0].threads.push({ id, name: `新しいスレッド ${created}`, agent: agent.name, model: label(settings.model), effort: label(settings.effort), turn_id: turn, running: false, tokens_used: 0 });
      details[id] = { id, turn_id: turn, entries: [], permission: null, question: null, settings };
      return { thread_id: id };
    }
    const target = details[params.thread_id] ?? detail;
    if (params.turn_id !== target.turn_id) throw new Error('stale_turn');
    if (method === 'remote_set_setting') {
      last.set_setting = { setting: params.setting, value: params.value };
      const setting = target.settings?.[params.setting];
      if (!setting?.choices.some(choice => choice.id === params.value && choice.allowed !== false)) throw new Error('setting_unavailable');
      setting.current = params.value;
      const thread = snapshot.projects[0].threads.find(candidate => candidate.id === target.id);
      if (thread && (params.setting === 'model' || params.setting === 'effort')) thread[params.setting] = setting.choices.find(choice => choice.id === params.value).label;
    } else if (method === 'remote_send_message') { detail.entries.push({ kind: 'user', text: params.message }); counter++; }
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
    else if (url.pathname === '/last') result = structuredClone(last);
    res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify(result));
  } catch (error) { res.writeHead(500); res.end(error.message); }
});
server.listen(8792, '127.0.0.1');
function stop() { clearInterval(timer); bridge.stop(); server.close(); }
process.once('SIGTERM', stop); process.once('SIGINT', stop);
