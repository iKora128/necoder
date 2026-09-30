import { initiate, finish, random, validSecret, unbase64 } from './crypto.mjs';
import { saveDevice, devices, removeDevice } from './storage.mjs';
import { t, language, setLanguage } from './i18n.mjs';
import { markdown, plain } from './markdown.mjs';
import { describeStep, KINDS } from './steps.mjs';

const $ = id => document.getElementById(id);
const initialFragment = location.hash;
history.replaceState(null, '', location.pathname); // QR の秘密を履歴/後続リンクへ残さない。
history.scrollRestoration = 'manual';
let known = [], device, socket, channel, initiation, snapshot, detail, selectedTask = '', selectedThread = '';
let generation = 0, retry, attempts = 0, authenticatedAt = 0, connectingAt = 0, status = 'offline', pendingMutation = false;
let outbound = Promise.resolve(), inbound = Promise.resolve(), vapid, pushEnabled = false, renderedPermission = '', permissionReviewed = false;
const pending = new Map();
const drafts = new Map();
let draftKey = '';
// 画面は 2 枚: view = 'home'（共有中の全スレッド一覧）| 'thread'（1 スレッドの会話）。
let view = 'home', restoreLast = true, stickToLatest = true, homeScroll = 0;
const expanded = new Set(); // 開いた折り畳み（`thread:key`）。更新で描き直しても開いたままにする。
const SPINNER = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏';
let spinnerFrame = 0;
// 状態は色相でなく形と動きで出す（UI-SPEC §1.3）: 回答待ち = 半円の脈動 / 実行中 = 点字スピナー / 新着 = リング。
const GLYPH = { blocked: '◐', done: '○', lost: '○', idle: '●' };
const STATE_LABEL = { blocked: 'blocked', running: 'running', done: 'done', lost: 'lost', idle: 'idle' };
// 端末ごとの表示状態（秘密ではない）。最後に開いたスレッドと、スレッドごとに見た最後のターン。
const LAST = 'necoder-remote-last', SEEN = 'necoder-remote-seen';
function stored(key, fallback) { try { return JSON.parse(localStorage.getItem(key)) ?? fallback; } catch { return fallback; } }
function store(key, value) { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* 保存できなくても表示は続ける */ } }
let seen = stored(SEEN, {});

function node(tag, content, className) {
  const element = document.createElement(tag);
  if (content !== undefined) element.textContent = content;
  if (className) element.className = className;
  return element;
}
/// 識別色は GUI から届いた `#rrggbb` の時だけ差す。無ければ中立（CSS の既定）に戻す。差したかを返す。
function paint(element, variable, color) {
  const valid = typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color);
  if (valid) element.style.setProperty(variable, color);
  else element.style.removeProperty(variable);
  return valid;
}
/// `#rrggbb` を下地に混ぜる（上端バーの面の色。iOS / Safari の上端の色 theme-color にも同じ値を渡す）。
function mix(color, base, weight) {
  const channels = hex => [1, 3, 5].map(index => parseInt(hex.slice(index, index + 2), 16));
  const [top, bottom] = [channels(color), channels(base)];
  return `#${top.map((value, index) => Math.round(value * weight + bottom[index] * (1 - weight)).toString(16).padStart(2, '0')).join('')}`;
}
const BASE = '#16181e', TINT = 0.22; // style.css の --bg0 と、上端バーに混ぜるスレッド色の割合（揃えること）
function themeColor(color) { document.querySelector('meta[name=theme-color]').content = color; }
let noticeTimer;
function notice(message) {
  clearTimeout(noticeTimer);
  $('notice').textContent = message; $('notice').hidden = !message;
  if (message) noticeTimer = setTimeout(() => { $('notice').hidden = true; }, 8000);
}
function staticText() {
  document.querySelectorAll('[data-t]').forEach(element => element.textContent = t(element.dataset.t));
  document.querySelectorAll('[data-aria]').forEach(element => element.setAttribute('aria-label', t(element.dataset.aria)));
  $('language').value = language; $('language-settings').value = language;
}
function live() { return channel && snapshot && status === 'online' && Date.now() - authenticatedAt < 30_000 && !pendingMutation; }
function currentProject() { return snapshot?.projects.find(project => project.id === selectedTask); }
function currentThread() { return currentProject()?.threads.find(thread => thread.id === selectedThread); }
function controls() {
  document.body.dataset.status = status;
  $('status').textContent = t(status);
  $('settings-status').textContent = t(status);
  $('connection-banner').textContent = t(status);
  $('connection-banner').hidden = status === 'online';
  const thread = currentThread();
  const blocked = !!detail?.permission || !!thread?.question_pending;
  $('send').disabled = !live() || !detail || blocked || !!thread?.running;
  $('interrupt').hidden = !thread?.running;
  $('interrupt').disabled = !live();
  $('diff').disabled = !live();
  $('message').placeholder = t(thread?.running ? 'composerRunning' : blocked ? 'composerBlocked' : 'message');
  $('notifications').disabled = !channel || status !== 'online';
  $('notifications').textContent = t(pushEnabled ? 'notificationsOff' : 'notifications');
  document.querySelectorAll('.project-head .add').forEach(button => { button.disabled = !live(); });
  document.querySelectorAll('[data-mutation]').forEach(button => {
    button.disabled = !live() || (button.dataset.kind === 'allow' && !permissionReviewed);
  });
}
function changeStatus(value) { status = value; controls(); }
function showView() {
  $('welcome').hidden = !!device;
  $('home').hidden = !device || view !== 'home';
  $('thread-view').hidden = !device || view !== 'thread';
  $('to-latest').hidden = true;
}
function refreshHosts() {
  $('hosts').replaceChildren(...known.map(host => {
    const option = node('option', host.name || 'Mac'); option.value = host.room; return option;
  }));
  if (device) $('hosts').value = device.room;
  $('host-name').textContent = device?.name || '';
  showView();
}
function resetConnection() {
  generation++; clearTimeout(retry);
  if (socket) { socket.onclose = null; socket.close(); }
  channel = null; initiation = null; authenticatedAt = 0;
  for (const { reject, timer } of pending.values()) { clearTimeout(timer); reject(new Error('outcome_unknown_check_history')); }
  pending.clear(); pendingMutation = false; renderedPermission = ''; permissionReviewed = false;
  changeStatus('offline');
}
async function connect() {
  resetConnection();
  if (!device || document.hidden) return;
  const ownGeneration = generation;
  const selectedDevice = device;
  // 画面は消さない（読んでいた会話を残したまま、届いた最新で描き直す）。
  snapshot = null; detail = null;
  changeStatus('connecting');
  connectingAt = Date.now(); pushEnabled = false;
  const url = new URL(`/api/rooms/${device.room}/ws`, location.origin);
  url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  socket = new WebSocket(url, ['necoder-v1', `auth.${device.token}`]);
  const ownSocket = socket;
  outbound = Promise.resolve(); inbound = Promise.resolve();
  const guard = () => generation === ownGeneration && ownSocket === socket;
  socket.onopen = () => { if (guard()) changeStatus('authenticating'); };
  socket.onmessage = event => {
    if (!guard() || event.data === 'pong') return;
    inbound = inbound.then(async () => {
      if (!guard()) return;
      if (typeof event.data !== 'string' || event.data.length > 2_800_000) throw new Error('invalid_frame');
      const message = JSON.parse(event.data);
      if (message.type === 'peer') {
        // リレーの online は信頼しない。Mac の証明が確認できるまで操作不可。
        channel = null; initiation = null; changeStatus(message.online ? 'authenticating' : 'offline');
        if (message.online) {
          initiation = await initiate(selectedDevice.secret, selectedDevice.room);
          if (guard()) ownSocket.send(JSON.stringify(initiation.hello));
        }
        return;
      }
      if (message.type === 'welcome' && initiation) {
        const verified = await finish(selectedDevice.secret, selectedDevice.room, initiation, message);
        if (guard()) { channel = verified; initiation = null; }
        return;
      }
      if (!channel) throw new Error('authentication_required');
      const value = await channel.open(message);
      if (!guard()) return;
      authenticatedAt = Date.now();
      attempts = 0;
      if (value.type === 'paired') {
        if (!validSecret(value.secret) || value.room !== selectedDevice.room) throw new Error('invalid_pairing');
        selectedDevice.secret = value.secret; selectedDevice.name = String(value.name || 'Mac').slice(0, 80); selectedDevice.paired = true;
        try { await saveDevice(selectedDevice); } catch { throw new Error('storage_failed'); }
        known = await devices(); device = selectedDevice; refreshHosts();
        await send({ type: 'confirm' });
      } else if (value.type === 'ready') {
        if (!selectedDevice.paired) throw new Error('pairing_not_saved');
        // 保存後 confirm 前に切断しても、新しい接続で確認を完了できる。
        await send({ type: 'confirm' });
      } else if (value.type === 'snapshot') {
        snapshot = value.snapshot; vapid = value.vapid; pushEnabled = value.push_subscribed;
        changeStatus(value.online && snapshot ? 'online' : 'guiOffline');
        $('last-seen').textContent = `${t('lastSeen')}: ${new Date().toLocaleTimeString()}`;
        render();
      } else if (value.type === 'thread') {
        // ホストが押してくる更新は、選択時と同じ instance の時だけ届く。
        if (value.task_id === selectedTask && value.detail.id === selectedThread) { detail = value.detail; detailInstance = snapshot?.instance_id; renderDetail(); }
      } else if (value.type === 'response') {
        const entry = pending.get(value.id);
        if (entry) { clearTimeout(entry.timer); pending.delete(value.id); value.ok ? entry.resolve(value.result) : entry.reject(new Error(value.error)); }
      } else if (value.type === 'push_saved') { pushEnabled = true; notice(t('pushReady')); }
      else if (value.type === 'push_removed') { pushEnabled = false; notice(t('pushOff')); }
      controls();
    }).catch(error => { if (guard()) { showError(error); ownSocket.close(1008, 'invalid_session'); } });
  };
  socket.onclose = event => {
    if (!guard()) return;
    resetConnection();
    if (event.code === 4003) { notice(t('pairExpired')); return; }
    if (!selectedDevice.paired) { notice(t('pairLost')); return; }
    if (!document.hidden) retry = setTimeout(() => connect().catch(showError), Math.min(30_000, 1000 * 2 ** Math.min(5, attempts++)) + Math.random() * 700);
  };
  socket.onerror = () => {}; // close で再接続する。
}
function send(value) {
  const currentChannel = channel, currentSocket = socket;
  if (!currentChannel || currentSocket?.readyState !== WebSocket.OPEN) return Promise.reject(new Error('offline'));
  const operation = outbound.then(async () => {
    if (channel !== currentChannel) throw new Error('offline');
    const encrypted = await currentChannel.seal(value);
    if (channel !== currentChannel || currentSocket.readyState !== WebSocket.OPEN) throw new Error('offline');
    currentSocket.send(JSON.stringify(encrypted));
  });
  outbound = operation.catch(() => {}); return operation;
}
function request(method, params) {
  const id = random();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error('outcome_unknown_check_history')); }, 15_000);
    pending.set(id, { resolve, reject, timer });
    send({ type: 'request', id, method, params, expires_at: Date.now() + 25_000 }).catch(error => {
      clearTimeout(timer); pending.delete(id); reject(error);
    });
  });
}
function parameters() {
  return { instance_id: snapshot?.instance_id, task_id: selectedTask, thread_id: selectedThread, turn_id: detail?.turn_id };
}
async function mutate(method, extra = {}) {
  if (!live()) throw new Error('offline');
  const params = { ...parameters(), ...extra };
  pendingMutation = true; controls(); $('receipt').textContent = t('sending');
  try {
    const result = await request(method, params);
    $('receipt').textContent = t('accepted');
    await request('snapshot', {}).then(() => loadThread()).catch(showError);
    return result;
  } catch (error) { $('receipt').textContent = t('unknown'); throw error; }
  finally { pendingMutation = false; controls(); }
}

// ── 描画: snapshot（一覧）───────────────────────────────────────────────────
function render() {
  if (snapshot) rememberSeen();
  if (view === 'thread' && snapshot) {
    if (!currentThread()) closeThread();
    // GUI が再起動した（instance が変わった）後の会話は古いターンを指すので取り直す。
    else { renderThreadHeader(); if (!detail || detailInstance !== snapshot.instance_id) loadThread().catch(showError); }
  } else if (view === 'home' && snapshot && restoreLast) {
    // 前回開いていた会話へ戻す（iOS はバックグラウンドの PWA をよく破棄するので、開き直すたびに一覧からだと遠い）。
    restoreLast = false;
    const last = stored(LAST, null);
    const project = snapshot.projects.find(candidate => candidate.id === last?.task);
    if (last?.room === device?.room && project?.threads.some(thread => thread.id === last.thread)) openThread(last.task, last.thread);
  }
  renderHome();
  controls();
}
/// 端末で見た最後のターンを覚える（開いている会話は常に既読）。見たことのない
/// スレッドは初回に既読扱いで登録する（ペアリング直後に全部が「新着」にならないように）。
function rememberSeen() {
  const prefix = `${device.room}:`;
  const next = Object.fromEntries(Object.entries(seen).filter(([key]) => !key.startsWith(prefix)));
  for (const project of snapshot.projects) for (const thread of project.threads) {
    const key = prefix + thread.id;
    next[key] = view === 'thread' && thread.id === selectedThread ? thread.turn_id : seen[key] ?? thread.turn_id;
  }
  seen = next; store(SEEN, seen);
}
function threadState(thread) {
  if (thread.blocked) return 'blocked';
  if (thread.running) return 'running';
  if (thread.session_lost) return 'lost';
  return seen[`${device.room}:${thread.id}`] !== thread.turn_id ? 'done' : 'idle';
}
function glyph(state) {
  return node('span', state === 'running' ? SPINNER[spinnerFrame] : GLYPH[state], `glyph ${state}`);
}
function compact(tokens) {
  if (!tokens) return '';
  if (tokens < 1000) return String(tokens);
  if (tokens < 1_000_000) return `${Math.round(tokens / 1000)}k`;
  return `${(tokens / 1_000_000).toFixed(1)}M`;
}
function renderHome() {
  const list = $('home-list');
  if (!snapshot) {
    if (status === 'guiOffline') list.replaceChildren(node('p', t('guiOfflineHint'), 'empty'));
    return; // 接続し直している間は前の一覧を残す
  }
  const projects = snapshot.projects;
  if (!projects.length) { list.replaceChildren(node('p', t('notShared'), 'empty')); return; }
  const blocks = [];
  const attention = projects.flatMap(project => project.threads.filter(thread => thread.blocked).map(thread => ({ project, thread })));
  if (attention.length) {
    const title = node('div', t('attention'), 'group-title'); title.append(node('span', String(attention.length), 'count'));
    blocks.push(title, ...attention.map(item => threadRow(item, true)));
  }
  // 動いているプロジェクトを上へ（同じ重みの中はデスクトップのレール順のまま）。
  const weight = project => project.threads.some(thread => thread.blocked || thread.running) ? 0
    : project.threads.some(thread => threadState(thread) === 'done') ? 1 : 2;
  // プロジェクトごとに 1 つの塊にし、左端をプロジェクト色の縦線でつなぐ（デスクトップのレールと同じ色）。
  for (const project of [...projects].sort((a, b) => weight(a) - weight(b))) {
    const section = node('section', undefined, 'project');
    if (paint(section, '--project', project.color)) section.classList.add('colored');
    section.append(projectHead(project));
    if (!project.threads.length) section.append(node('p', t('emptyProject'), 'empty-row'));
    for (const thread of project.threads) section.append(threadRow({ project, thread }, false));
    blocks.push(section);
  }
  list.replaceChildren(...blocks);
}
/// どこで動いているか（プロジェクト ⎇ ブランチ ＠リモートのホスト）。
function whereText(project) {
  return [project.branch ? `⎇ ${project.branch}` : '', project.remote_host ? `@${project.remote_host}` : ''].filter(Boolean).join('  ');
}
function projectHead(project) {
  const head = node('div', undefined, 'project-head');
  head.append(node('span', project.name, 'name'), node('span', whereText(project), 'branch'));
  // 新規スレッドは既存スレッドと同じパネルに作る（GUI はスレッド ID でパネルを引く）ので、1 本も無い所には出さない。
  const anchor = project.threads[0];
  if (anchor) {
    const add = node('button', '＋', 'add'); add.type = 'button'; add.setAttribute('aria-label', t('newThread'));
    add.onclick = () => chooseAgent(project, anchor);
    head.append(add);
  }
  return head;
}
function threadRow({ project, thread }, withProject) {
  const state = threadState(thread);
  const row = node('button', undefined, state === 'done' ? 'row unseen' : 'row');
  row.type = 'button';
  row.dataset.threadName = thread.name;
  paint(row, '--thread', thread.color);
  row.append(glyph(state), node('span', thread.name || t('untitled'), 'name'), node('span', compact(thread.tokens_used), 'tokens'));
  const meta = node('span', undefined, 'meta');
  // 「要対応」の枠ではプロジェクトの塊の外に出るので、どのプロジェクトかを色つきで添える。
  if (withProject) {
    const chip = node('span', project.name, 'project-chip');
    if (paint(chip, '--project', project.color)) chip.classList.add('colored');
    meta.append(chip, ' · ');
  }
  meta.append(thread.agent);
  if (state === 'idle') meta.append(` · ${t('idle')}`);
  else { meta.append(' · '); meta.append(node('b', t(STATE_LABEL[state]))); }
  row.append(meta);
  const preview = thread.digest ? plain(thread.digest) : '';
  if (preview) row.append(node('span', preview, 'preview'));
  row.onclick = () => openThread(project.id, thread.id);
  return row;
}

// ── 画面遷移 ───────────────────────────────────────────────────────────────
function saveDraft() { if (draftKey) drafts.set(draftKey, $('message').value); }
function openThread(task, thread) {
  saveDraft();
  if (view === 'home') homeScroll = window.scrollY;
  selectedTask = task; selectedThread = thread; detail = null; stickToLatest = true;
  renderedPermission = ''; permissionReviewed = false;
  draftKey = `${device?.room}:${task}:${thread}`;
  $('message').value = drafts.get(draftKey) || '';
  if (view !== 'thread') history.pushState({ thread }, '');
  view = 'thread';
  store(LAST, { room: device?.room, task, thread });
  if (snapshot) rememberSeen();
  $('transcript').replaceChildren(); $('permission').replaceChildren(); $('question').replaceChildren(); $('receipt').textContent = '';
  renderThreadHeader(); showView(); autosize(); window.scrollTo(0, 0); controls();
  loadThread().catch(showError);
}
function closeThread() {
  saveDraft(); draftKey = '';
  view = 'home'; selectedThread = ''; detail = null;
  store(LAST, null); themeColor(BASE);
  showView(); renderHome(); controls();
  window.scrollTo(0, homeScroll);
}
function renderThreadHeader() {
  const project = currentProject(), thread = currentThread();
  if (!project || !thread) return;
  // 上端バー＝このスレッドのタブ。スレッド色の面と下線で「今どの会話か」を出す（色が届かない版では中立のまま）。
  const colored = paint($('thread-view'), '--thread', thread.color);
  $('thread-view').classList.toggle('colored', colored);
  themeColor(colored ? mix(thread.color, BASE, TINT) : BASE);
  $('thread-project').classList.toggle('colored', paint($('thread-project'), '--project', project.color));
  $('thread-name').textContent = thread.name || t('untitled');
  $('thread-project').textContent = [project.name, whereText(project)].filter(Boolean).join('  ');
  $('thread-meta').textContent = [thread.agent, thread.tokens_used ? `${thread.tokens_used.toLocaleString()} tokens` : ''].filter(Boolean).map(text => ` · ${text}`).join('');
  // 他で承認・質問を待っているスレッドの数を戻るボタンに出す（ここに居ても気付けるように）。
  const others = snapshot.projects.flatMap(candidate => candidate.threads).filter(candidate => candidate.blocked && candidate.id !== thread.id).length;
  $('back-badge').hidden = !others; $('back-badge').textContent = String(others);
}
let loadingThread = '', detailInstance = '';
async function loadThread() {
  if (!snapshot || !selectedThread || !channel) return;
  const key = `${generation}:${selectedTask}:${selectedThread}`;
  if (loadingThread === key) return;
  loadingThread = key;
  const instance = snapshot.instance_id;
  try {
    const result = await request('thread', parameters());
    if (key === `${generation}:${selectedTask}:${selectedThread}`) { detail = result; detailInstance = instance; renderDetail(); }
  } finally { if (loadingThread === key) loadingThread = ''; }
}

// ── 描画: 会話 ─────────────────────────────────────────────────────────────
function nearBottom() { return document.documentElement.scrollHeight - (window.scrollY + window.innerHeight) < 160; }
function scrollToLatest(behavior = 'auto') { window.scrollTo({ top: document.documentElement.scrollHeight, behavior }); }
function renderDetail() {
  if (!detail || view !== 'thread') return;
  const stick = stickToLatest || nearBottom();
  const anchor = stick ? null : visibleAnchor();
  renderTranscript();
  renderPermission();
  renderQuestion();
  controls();
  if (stick) scrollToLatest();
  else if (anchor) keepAnchor(anchor);
  stickToLatest = false;
}
/// 遡って読んでいる間の更新で読んでいる行を動かさない。古い項目は直近 60 件の窓から押し出されて
/// 上の高さが変わるので、描き直す前に画面上端の項目と位置を覚え、描き直した後に同じ位置へ戻す。
function visibleAnchor() {
  const top = $('thread-view').querySelector('.bar').getBoundingClientRect().bottom;
  for (const element of $('transcript').children) {
    const rect = element.getBoundingClientRect();
    if (rect.bottom > top) return { key: element.dataset.key, offset: rect.top };
  }
  return null;
}
function keepAnchor(anchor) {
  const element = [...$('transcript').children].find(child => child.dataset.key === anchor.key);
  if (element) window.scrollBy(0, element.getBoundingClientRect().top - anchor.offset);
}
const isWork = entry => entry.kind === 'tool' || entry.kind === 'thinking';
function renderTranscript() {
  const running = !!currentThread()?.running;
  const entries = detail.entries;
  const blocks = [];
  // 各ブロックに項目 ID の鍵を付ける（描き直しを跨いで同じ行を探すため・visibleAnchor）。
  const push = (block, key) => { block.dataset.key = key; blocks.push(block); };
  if (detail.history_truncated) push(node('p', t('history'), 'history'), 'history');
  if (!entries.length) push(node('p', t('emptyThread'), 'history'), 'empty');
  for (let index = 0; index < entries.length;) {
    if (isWork(entries[index])) {
      const run = [];
      while (index < entries.length && isWork(entries[index])) run.push(entries[index++]);
      push(workBlock(run, running && index === entries.length), `work:${run[0].id}`);
      continue;
    }
    const entry = entries[index++];
    push(entryBlock(entry), `entry:${entry.id}`);
  }
  if (running && (!entries.length || !isWork(entries.at(-1)))) {
    const row = node('div', undefined, 'working');
    row.append(bullet(true), node('span', t('running')));
    push(row, 'running');
  }
  $('transcript').replaceChildren(...blocks);
}
function bullet(live) { return live ? node('span', SPINNER[spinnerFrame], 'bullet glyph running') : node('span', '⏺', 'bullet'); }
// 種類の印（自前の単純な線画・色は currentColor＝中立。識別色は使わない）。
const SVG = 'http://www.w3.org/2000/svg';
const ICONS = {
  read: ['M7 3h7l4 4v14H7z', 'M14 3v4h4', 'M10 12h5', 'M10 16h5'],
  search: ['M16 10.5a5.5 5.5 0 1 1-11 0a5.5 5.5 0 1 1 11 0', 'M14.5 14.5L20 20'],
  edit: ['M5 19l1-4L16 5l3 3L9 18z', 'M14 7l3 3'],
  write: ['M7 3h7l4 4v14H7z', 'M14 3v4h4', 'M12.5 11v6', 'M9.5 14h6'],
  web: ['M21 12a9 9 0 1 1-18 0a9 9 0 1 1 18 0', 'M3 12h18', 'M12 3c3 3.2 3 14.8 0 18', 'M12 3c-3 3.2-3 14.8 0 18'],
  tool: ['M12 3l8 4.5v9L12 21l-8-4.5v-9z', 'M4 7.5l8 4.5 8-4.5', 'M12 12v9'],
  run: ['M5 7l5 5-5 5', 'M12 18h7'],
};
function icon(kind) {
  const svg = document.createElementNS(SVG, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'icon'); svg.setAttribute('aria-hidden', 'true');
  for (const shape of ICONS[kind]) { const path = document.createElementNS(SVG, 'path'); path.setAttribute('d', shape); svg.append(path); }
  return svg;
}
/// 押すと開閉する 1 行（中身は開いた時だけ作る＝閉じている間は長い出力を DOM に持たない）。
function foldable(key, className, parts, build, bodyClass) {
  const wrapper = node('div');
  const id = `${selectedThread}:${key}`;
  if (!build) { const row = node('div', undefined, className); row.append(...parts); wrapper.append(row); return wrapper; }
  const head = node('button', undefined, className); head.type = 'button';
  const body = node('div', undefined, bodyClass);
  const apply = () => {
    const open = expanded.has(id);
    head.setAttribute('aria-expanded', String(open));
    body.hidden = !open;
    body.replaceChildren(...(open ? build() : []));
  };
  head.append(...parts, node('span', '▸', 'chevron'));
  head.onclick = () => { if (expanded.has(id)) expanded.delete(id); else expanded.add(id); apply(); };
  apply();
  wrapper.append(head, body);
  return wrapper;
}
/// 「読む mcp.rs crates/acp_client/src」のように、動詞・対象・場所を並べる。
function stepParts(step, lead) {
  const target = node('span', step.target || '', step.code ? 'target code' : 'target');
  if (step.place) target.append(node('span', step.place, 'place'));
  return [...lead, icon(step.kind), node('span', t(`kind_${step.kind}`), 'verb'), target];
}
function stepRow(entry, live) {
  const step = describeStep(entry.text);
  const parts = stepParts(step, live ? [bullet(true)] : []);
  if (!step.rest) return foldable(`step:${entry.id}`, 'step', parts);
  parts.push(node('span', t('lines').replace('{n}', step.rest.split('\n').length), 'lines'));
  return foldable(`step:${entry.id}`, 'step', parts, () => [node('pre', step.rest, 'result')]);
}
function thoughtRow(entry) {
  const preview = entry.text.split('\n').find(line => line.trim()) ?? '';
  return foldable(`thought:${entry.id}`, 'step', [node('span', '✳', 'mark'), node('span', t('thinking'), 'verb'), node('span', preview, 'target prose')],
    () => [node('div', entry.text, 'thought')]);
}
/// 連続したツール実行と思考を 1 つの箱（作業の欄）にまとめる。閉じた見出しは「何を何回したか」、
/// 実行中は「いま何をしているか」を出す。生のコマンドは開いた先で見る。
function workBlock(run, live) {
  const box = node('div', undefined, live ? 'workbox live' : 'workbox');
  const row = entry => entry.kind === 'tool' ? stepRow(entry, false) : thoughtRow(entry);
  if (run.length === 1) {
    box.append(run[0].kind === 'tool' ? stepRow(run[0], live) : thoughtRow(run[0]));
    return box;
  }
  const steps = run.filter(entry => entry.kind === 'tool');
  let parts;
  if (live && steps.length) parts = stepParts(describeStep(steps.at(-1).text), [bullet(true)]);
  else {
    const counts = {};
    for (const entry of run) { const kind = entry.kind === 'tool' ? describeStep(entry.text).kind : 'think'; counts[kind] = (counts[kind] ?? 0) + 1; }
    const kinds = node('span', undefined, 'kinds');
    for (const kind of [...KINDS, 'think']) {
      if (!counts[kind]) continue;
      const chip = node('span', undefined, 'kind');
      chip.append(kind === 'think' ? node('span', '✳', 'mark') : icon(kind), node('span', t(`kind_${kind}`)), node('b', String(counts[kind])));
      kinds.append(chip);
    }
    parts = [bullet(live), node('span', t('steps').replace('{n}', run.length), 'count'), kinds];
  }
  box.append(foldable(`work:${run[0].id}`, 'work-head', parts, () => run.map(row), 'work-list'));
  return box;
}
function entryBlock(entry) {
  if (entry.kind === 'user') return userBlock(entry);
  if (entry.kind === 'agent') { const box = node('div', undefined, 'msg-agent'); box.append(markdown(entry.text)); return box; }
  if (entry.kind === 'auto_prompt') {
    // necoder の知らせ（Captain への台帳の通知など）。人の発話と混ぜず灰色のカードで出す。1 行目が出所と時刻。
    const [label, ...body] = entry.text.split('\n');
    const card = node('div', undefined, 'msg-auto');
    card.append(node('div', `${t('auto_prompt')} · ${label}`, 'msg-auto-head'), node('div', body.join('\n').trim()));
    return card;
  }
  return node('p', entry.kind === 'checkpoint' ? `◇ ${t('checkpoint')}: ${entry.text}` : entry.text, 'marker');
}
/// 長い入力（>12 行 or >1000 文字）は先頭だけ見せる（UI-SPEC §6 msg-user と同じ閾値）。
function userBlock(entry) {
  const box = node('div', undefined, 'msg-user');
  const lines = entry.text.split('\n');
  const long = lines.length > 12 || entry.text.length > 1000;
  const id = `${selectedThread}:user:${entry.id}`;
  const draw = () => {
    const open = expanded.has(id);
    const text = !long || open ? entry.text : `${lines.length > 12 ? lines.slice(0, 8).join('\n') : entry.text.slice(0, 400)}…`;
    box.replaceChildren(document.createTextNode(text));
    if (!long) return;
    const more = node('button', open ? t('collapse') : lines.length > 12 ? t('showAll').replace('{n}', lines.length) : t('showMore'), 'more');
    more.type = 'button';
    more.onclick = () => { if (open) expanded.delete(id); else expanded.add(id); draw(); };
    box.append(more);
  };
  draw();
  return box;
}
function cardEyebrow(label) {
  const eyebrow = node('div', undefined, 'card-eyebrow');
  eyebrow.append(glyph('blocked'), node('span', label));
  return eyebrow;
}
function renderPermission() {
  const permission = detail.permission;
  if (renderedPermission !== permission?.id) permissionReviewed = false;
  renderedPermission = permission?.id || '';
  if (!permission) { $('permission').replaceChildren(); return; }
  // 同じ承認は描き直さない（読んでいる位置とチェックを保つ）。
  if ($('permission').firstElementChild?.dataset.id === permission.id) return;
  const card = node('section', undefined, 'card');
  card.dataset.id = permission.id;
  card.append(cardEyebrow(t('permission')), node('h3', permission.title));
  if (!permission.complete) card.append(node('p', t('incomplete'), 'hint'));
  if (permission.raw_input) card.append(node('pre', permission.raw_input));
  for (const diff of permission.diffs) {
    const section = node('details'); section.open = true;
    section.append(node('summary', diff.path), node('p', t('before'), 'caption'), node('pre', diff.old_text ?? ''), node('p', t('after'), 'caption'), node('pre', diff.new_text));
    card.append(section);
  }
  if (permission.complete) {
    const label = node('label', undefined, 'check'), checkbox = node('input'); checkbox.type = 'checkbox'; checkbox.checked = permissionReviewed;
    checkbox.onchange = () => { permissionReviewed = checkbox.checked; controls(); };
    label.append(checkbox, node('span', t('reviewed'))); card.append(label);
  }
  const buttons = node('div', undefined, 'choices');
  // 拒否を左・許可を右（押し間違えても危なくない側を先に）。
  for (const option of [...permission.options].sort((a, b) => (a.kind === 'allow') - (b.kind === 'allow'))) {
    const button = node('button', t(option.kind === 'allow' ? 'approve' : 'reject'), option.kind === 'allow' ? 'allow' : undefined);
    button.type = 'button'; button.dataset.mutation = 'true'; button.dataset.kind = option.kind;
    button.onclick = () => mutate('permission_response', { permission_id: permission.id, option_id: option.id }).catch(showError);
    buttons.append(button);
  }
  card.append(buttons);
  $('permission').replaceChildren(card);
}
// 複数選択フィールドは配列、単一選択は文字列で集める（ホスト側 question_response が両方受ける）。
// FormData をそのまま Object.fromEntries すると複数選択が最後の 1 件に潰れる。
function questionSelections(form, question) {
  const data = new FormData(form);
  const selections = {};
  for (const field of question?.fields ?? []) {
    selections[field.name] = field.multi ? data.getAll(field.name) : (data.get(field.name) ?? '');
  }
  return selections;
}
// 自分で書いた答え（Other 欄つきの質問だけ・空は入れない）。ホスト側で Other 欄の名前に直して返す（O17）。
function questionCustom(form, question) {
  const data = new FormData(form);
  const custom = {};
  for (const field of question?.fields ?? []) {
    const text = field.custom ? String(data.get(`custom:${field.name}`) ?? '').trim() : '';
    if (text) custom[field.name] = text;
  }
  return custom;
}
function renderQuestion() {
  const question = detail.question;
  if (!question) {
    $('question').replaceChildren(...(detail.question_pending ? [node('p', t('questionLocal'), 'card')] : []));
    return;
  }
  // 同じ質問は描き直さない（選びかけ・書きかけと、書いている欄のフォーカスを保つ）。
  if ($('question').firstElementChild?.dataset.id === question.id) return;
  const form = node('form', undefined, 'card');
  form.dataset.id = question.id;
  form.append(cardEyebrow(t('question')), node('p', question.message, 'message'));
  for (const field of question.fields) {
    if (!field.choices.length && !field.custom) continue;
    const fieldset = node('fieldset');
    fieldset.append(node('legend', field.title || field.name));
    for (const choice of field.choices) {
      const label = node('label', undefined, 'option'), input = node('input');
      input.type = field.multi ? 'checkbox' : 'radio'; input.name = field.name; input.value = choice.value;
      label.append(input, node('span', choice.label));
      fieldset.append(label);
    }
    if (field.custom) {
      const input = node('input', undefined, 'custom'); input.type = 'text'; input.name = `custom:${field.name}`; input.maxLength = 4000;
      input.placeholder = t(field.multi ? 'customMulti' : 'customSingle');
      fieldset.append(input);
    }
    form.append(fieldset);
  }
  const buttons = node('div', undefined, 'choices');
  const decline = node('button', t('decline')); decline.type = 'button'; decline.dataset.mutation = 'true';
  decline.onclick = () => mutate('question_response', { question_id: question.id, selections: null }).catch(showError);
  const submit = node('button', t('answer'), 'allow'); submit.type = 'submit'; submit.dataset.mutation = 'true';
  buttons.append(decline, submit);
  form.append(buttons);
  form.onsubmit = event => {
    event.preventDefault();
    // 質問ごとに「選ぶ」か「書く」のどちらかが要る（書いて答えられる質問は選ばなくてよい）。
    const selections = questionSelections(form, question), custom = questionCustom(form, question);
    const picked = value => Array.isArray(value) ? value.length > 0 : !!value;
    if (!question.fields.every(field => picked(selections[field.name]) || custom[field.name])) { notice(t('answerRequired')); return; }
    mutate('question_response', { question_id: question.id, selections, custom }).catch(showError);
  };
  $('question').replaceChildren(form);
}
function showError(error) {
  const message = error.message;
  notice(t(message === 'storage_failed' ? 'storageError' : message.includes('unknown') ? 'unknown'
    : message.startsWith('stale') || message === 'host_state_changed' ? 'stale'
    : message === 'authentication_failed' ? 'pairLost' : message === 'offline' ? 'offline' : message));
}
async function pairing(fragment) {
  const params = new URLSearchParams(fragment.replace(/^#/, ''));
  if (!['room', 'token', 'secret'].every(key => validSecret(params.get(key)))) throw new Error(t('pairInvalid'));
  const expires = Number(params.get('expires'));
  if (!Number.isSafeInteger(expires) || expires < Date.now() || expires > Date.now() + 360_000) throw new Error(t('pairExpired'));
  device = { room: params.get('room'), token: params.get('token'), secret: params.get('secret'), paired: false, name: 'Mac' };
  refreshHosts(); notice(''); await connect();
}
$('pair-form').onsubmit = event => {
  event.preventDefault();
  try {
    const url = new URL($('pair-url').value);
    if (url.origin !== location.origin) throw new Error(t('pairInvalid'));
    $('pair-url').value = ''; pairing(url.hash).catch(showError);
  } catch (error) { showError(error); }
};
// ── QR スキャン ─────────────────────────────────────────────────────────────
// iOS のカメラアプリで QR を読むと **Safari** が開き、ホーム画面 PWA とは保存領域が別なので
// ペアリングが引き継がれない（iOS に PWA 向けの universal link は無い）。だから**アプリの中で**
// 読む。QR の中身はこの origin のペアリング URL 以外は一切受け付けない。
let scanStream = null, scanTimer = null, scanning = false;
function stopScan() {
  clearInterval(scanTimer); scanTimer = null; scanning = false;
  for (const track of scanStream?.getTracks() ?? []) track.stop();
  scanStream = null;
  $('scan-video').srcObject = null;
  if ($('scan-dialog').open) $('scan-dialog').close();
}
/// 読み取れた文字列がこの origin のペアリング URL なら fragment を返す（それ以外は null）。
function pairingFragment(text) {
  try {
    const url = new URL(text);
    return url.origin === location.origin && url.hash ? url.hash : null;
  } catch { return null; }
}
async function startScan() {
  $('scan-error').textContent = '';
  // navigator.mediaDevices は secure context 以外や一部の WebView では生えない。拒否と同じ扱い（貼り付けへ倒す）。
  if (!navigator.mediaDevices?.getUserMedia) throw new Error('camera_unavailable');
  scanStream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: { ideal: 'environment' } }, audio: false });
  const video = $('scan-video');
  video.srcObject = scanStream;
  await video.play();
  $('scan-dialog').showModal();
  // Safari は BarcodeDetector を持たないので、その時だけデコーダを取りに行く（起動時には読まない）。
  const detector = 'BarcodeDetector' in window ? new BarcodeDetector({ formats: ['qr_code'] }) : null;
  const decode = detector ? null : (await import('/jsqr.mjs')).default;
  const canvas = document.createElement('canvas');
  const context = canvas.getContext('2d', { willReadFrequently: true });
  scanTimer = setInterval(async () => {
    if (scanning || video.readyState < 2 || !video.videoWidth) return;
    scanning = true;
    try {
      let text = null;
      if (detector) text = (await detector.detect(video))[0]?.rawValue ?? null;
      else {
        canvas.width = Math.min(640, video.videoWidth);
        canvas.height = Math.round(canvas.width * video.videoHeight / video.videoWidth);
        context.drawImage(video, 0, 0, canvas.width, canvas.height);
        text = decode(context.getImageData(0, 0, canvas.width, canvas.height).data, canvas.width, canvas.height)?.data ?? null;
      }
      const fragment = text && pairingFragment(text);
      if (!fragment) return;
      stopScan();
      await pairing(fragment);
    } catch (error) { stopScan(); showError(error); }
    finally { scanning = false; }
  }, 250);
}
$('scan').onclick = () => startScan().catch(() => {
  // 権限拒否・カメラ無しは行き止まりにしない（貼り付けの導線を開いて見せる）。
  // **文言を先に出してから後始末する** — 後始末（トラック停止・dialog.close）が環境依存で
  // 転んでも、利用者には理由が残る。逆順だと黙って何も起きないボタンになる。
  $('scan-error').textContent = t('scanUnavailable');
  $('pair-manual').open = true;
  try { stopScan(); } catch { /* 後始末の失敗は見せない */ }
});
$('close-scan').onclick = () => stopScan();
$('scan-dialog').addEventListener('close', stopScan);

// ── 操作 ──────────────────────────────────────────────────────────────────
function autosize() {
  const box = $('message');
  box.style.height = 'auto';
  box.style.height = `${Math.min(box.scrollHeight + 3, Math.round(window.innerHeight * 0.38))}px`;
}
$('message').addEventListener('input', autosize);
$('back').onclick = () => { if (history.state?.thread) history.back(); else closeThread(); };
window.addEventListener('popstate', () => { if (view === 'thread') closeThread(); });
window.addEventListener('scroll', () => { $('to-latest').hidden = view !== 'thread' || nearBottom(); }, { passive: true });
$('to-latest').onclick = () => scrollToLatest('smooth');
$('notice').onclick = () => { $('notice').hidden = true; };
$('open-settings').onclick = () => { controls(); $('settings-dialog').showModal(); };
$('close-settings').onclick = () => $('settings-dialog').close();
$('close-agent').onclick = () => $('agent-dialog').close();
// シートは背景を押しても閉じる。
for (const dialog of document.querySelectorAll('dialog.sheet')) dialog.addEventListener('click', event => { if (event.target === dialog) dialog.close(); });
$('hosts').onchange = () => {
  device = known.find(candidate => candidate.room === $('hosts').value);
  if (view === 'thread') closeThread();
  refreshHosts(); $('settings-dialog').close(); connect().catch(showError);
};
$('reconnect').onclick = () => { $('settings-dialog').close(); connect().catch(showError); };
$('composer').onsubmit = event => {
  event.preventDefault(); const message = $('message').value, destination = draftKey;
  if (!message.trim()) return;
  stickToLatest = true;
  mutate('send_message', { message }).then(() => {
    if (draftKey === destination && $('message').value === message) { $('message').value = ''; autosize(); }
    if (drafts.get(destination) === message) drafts.delete(destination);
  }).catch(showError);
};
$('interrupt').onclick = () => mutate('interrupt').catch(showError);
let agentTarget = null;
function chooseAgent(project, anchor) {
  agentTarget = { project, anchor };
  $('agent-project').textContent = t('newThreadIn').replace('{project}', project.name);
  const agents = [...new Set(['Claude Code', 'Codex', ...snapshot.projects.flatMap(candidate => candidate.threads.map(thread => thread.agent)).filter(Boolean)])];
  $('agent-choices').replaceChildren(...agents.map(agent => {
    const button = node('button', agent); button.type = 'button';
    button.onclick = () => {
      $('agent-dialog').close();
      const target = agentTarget;
      mutate('new_thread', { agent, task_id: target.project.id, thread_id: target.anchor.id })
        .then(result => openThread(target.project.id, result.thread_id)).catch(showError);
    };
    return button;
  }));
  $('agent-dialog').showModal();
}
function renderDiff(text) {
  $('diff-text').replaceChildren(...text.split('\n').map(line => {
    const kind = /^(diff --git|\+\+\+|---)/.test(line) ? 'file' : line.startsWith('@@') ? 'hunk'
      : line.startsWith('+') ? 'add' : line.startsWith('-') ? 'del' : undefined;
    return node('span', `${line}\n`, kind);
  }));
}
$('diff').onclick = async () => {
  try { const result = await request('get_diff', parameters()); renderDiff(result.diff || t('noDiff')); $('diff-dialog').showModal(); }
  catch (error) { showError(error); }
};
$('close-diff').onclick = () => $('diff-dialog').close();
$('forget').onclick = async () => {
  if (!confirm(t('forgetConfirm'))) return;
  try {
    $('settings-dialog').close();
    resetConnection(); await removeDevice(device.room); known = await devices(); device = known[0];
    snapshot = null; detail = null; view = 'home'; selectedThread = ''; store(LAST, null);
    refreshHosts(); await connect();
  } catch (error) { showError(error); }
};
$('notifications').onclick = async () => {
  try {
    if (!('Notification' in window) || !('serviceWorker' in navigator) || !vapid) throw new Error(t('pushDenied'));
    if (pushEnabled) { await send({ type: 'unsubscribe' }); return; }
    if (await Notification.requestPermission() !== 'granted') throw new Error(t('pushDenied'));
    const registration = await navigator.serviceWorker.ready;
    let subscription = await registration.pushManager.getSubscription();
    // 各MacのVAPID鍵に対応する。複数Macで通知を使う場合は同じVAPID鍵の設定が必要。
    if (subscription && subscription.options.applicationServerKey
        && [...new Uint8Array(subscription.options.applicationServerKey)].join(',') !== [...unbase64(vapid)].join(',')) {
      await subscription.unsubscribe(); subscription = null;
    }
    subscription ||= await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: unbase64(vapid) });
    await send({ type: 'subscribe', subscription: subscription.toJSON() });
  } catch (error) { showError(error); }
};
function changeLanguage(value) {
  setLanguage(value); staticText(); renderHome();
  if (view === 'thread') {
    renderThreadHeader();
    // 承認・質問のカードは同じ ID だと描き直さないので、言語を変えた時だけ明示的に捨てる。
    $('permission').replaceChildren(); $('question').replaceChildren(); renderDetail();
  }
  controls();
}
$('language').onchange = () => changeLanguage($('language').value);
$('language-settings').onchange = () => changeLanguage($('language-settings').value);
document.addEventListener('visibilitychange', () => { document.hidden ? resetConnection() : connect().catch(showError); });
window.addEventListener('online', () => connect().catch(showError));
window.addEventListener('offline', () => resetConnection());
setInterval(() => {
  if (document.hidden) return;
  if (!channel && socket?.readyState < WebSocket.CLOSING && Date.now() - connectingAt > 30_000) { socket.close(); return; }
  if (channel && Date.now() - authenticatedAt > 30_000) { connect().catch(showError); return; }
  if (channel) send({ type: 'heartbeat' }).catch(showError);
  controls();
}, 10_000);
// 実行中の印（点字スピナー）を回す。描き直しで作る印も同じコマから始める。
setInterval(() => {
  if (document.hidden || matchMedia('(prefers-reduced-motion: reduce)').matches) return;
  const glyphs = document.querySelectorAll('.glyph.running');
  if (!glyphs.length) return;
  spinnerFrame = (spinnerFrame + 1) % SPINNER.length;
  for (const element of glyphs) element.textContent = SPINNER[spinnerFrame];
}, 125);
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(showError);
  navigator.serviceWorker.addEventListener('message', event => { if (event.data?.type === 'refresh') connect().catch(showError); });
}
staticText();
try {
  known = await devices(); device = known[0]; refreshHosts();
  if (initialFragment) await pairing(initialFragment); else if (device) await connect();
} catch (error) { showError(error); }
