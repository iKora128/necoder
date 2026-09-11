import { initiate, finish, random, validSecret, unbase64 } from './crypto.mjs';
import { saveDevice, devices, removeDevice } from './storage.mjs';
import { t, language, setLanguage } from './i18n.mjs';

const $ = id => document.getElementById(id);
const initialFragment = location.hash;
history.replaceState(null, '', location.pathname); // QR の秘密を履歴/後続リンクへ残さない。
let known = [], device, socket, channel, initiation, snapshot, detail, selectedTask = '', selectedThread = '';
let generation = 0, retry, attempts = 0, authenticatedAt = 0, connectingAt = 0, status = 'offline', pendingMutation = false;
let outbound = Promise.resolve(), inbound = Promise.resolve(), vapid, pushEnabled = false, renderedPermission = '', permissionReviewed = false;
const pending = new Map();
const drafts = new Map();
let draftKey = '';
function node(tag, content, className) {
  const element = document.createElement(tag);
  if (content !== undefined) element.textContent = content;
  if (className) element.className = className;
  return element;
}
function notice(message) { $('notice').textContent = message; $('notice').hidden = !message; }
function staticText() {
  document.querySelectorAll('[data-t]').forEach(element => element.textContent = t(element.dataset.t));
  $('language').value = language;
}
function live() { return channel && snapshot && status === 'online' && Date.now() - authenticatedAt < 30_000 && !pendingMutation; }
function controls() {
  $('status').textContent = t(status);
  const thread = snapshot?.projects.find(p => p.id === selectedTask)?.threads.find(th => th.id === selectedThread);
  $('send').disabled = !live() || !detail || !!detail.permission || !!thread?.question_pending || !!thread?.running;
  $('interrupt').disabled = !live() || !thread?.running;
  $('diff').disabled = !live(); $('new-thread').disabled = !live() || !selectedTask;
  $('notifications').disabled = !channel || status !== 'online';
  $('notifications').textContent = t(pushEnabled ? 'notificationsOff' : 'notifications');
  document.querySelectorAll('[data-mutation]').forEach(button => {
    button.disabled = !live() || (button.dataset.kind === 'allow' && !permissionReviewed);
  });
}
function changeStatus(value) { status = value; controls(); }
function refreshHosts() {
  $('hosts').replaceChildren(...known.map(host => {
    const option = node('option', host.name || 'Mac'); option.value = host.room; return option;
  }));
  if (device) $('hosts').value = device.room;
  $('welcome').hidden = !!device; $('control').hidden = !device;
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
  snapshot = null; detail = null; $('thread-view').hidden = true;
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
        renderProjects();
      } else if (value.type === 'thread') {
        if (value.task_id === selectedTask && value.detail.id === selectedThread) { detail = value.detail; renderDetail(); }
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
function renderProjects() {
  const projects = snapshot?.projects || [];
  const previousTask = selectedTask;
  if (!projects.some(p => p.id === selectedTask)) selectedTask = projects[0]?.id || '';
  $('projects').replaceChildren(...projects.map(project => {
    const option = node('option', `${project.name}${project.branch ? ' · ' + project.branch : ''}`);
    option.value = project.id; return option;
  }));
  $('projects').value = selectedTask;
  if (!projects.length && snapshot) notice(t('notShared'));
  const threads = projects.find(p => p.id === selectedTask)?.threads || [];
  const previousThread = selectedThread;
  if (!threads.some(th => th.id === selectedThread)) selectedThread = threads[0]?.id || '';
  const nextDraft = `${device?.room}:${selectedTask}:${selectedThread}`;
  if (draftKey !== nextDraft) {
    if (draftKey) drafts.set(draftKey, $('message').value);
    draftKey = nextDraft; $('message').value = drafts.get(draftKey) || '';
  }
  $('threads').replaceChildren(...threads.map(thread => {
    const button = node('button', thread.name, 'thread' + (thread.id === selectedThread ? ' active' : ''));
    button.setAttribute('aria-pressed', String(thread.id === selectedThread));
    button.append(node('small', `${thread.agent} · ${t(thread.blocked ? 'blocked' : thread.running ? 'running' : thread.session_lost ? 'lost' : 'idle')}`));
    button.onclick = () => { selectedThread = thread.id; detail = null; renderProjects(); loadThread().catch(showError); };
    return button;
  }));
  if (selectedThread && (!detail || previousTask !== selectedTask || previousThread !== selectedThread)) loadThread().catch(showError);
  $('thread-view').hidden = !selectedThread;
  const thread = threads.find(th => th.id === selectedThread);
  const project = projects.find(p => p.id === selectedTask);
  $('destination').textContent = project && thread ? `${project.name} ⎇ ${project.branch || '—'} / ${thread.name}` : '';
  $('tokens').textContent = thread ? `${thread.tokens_used.toLocaleString()} tokens` : '';
  controls();
}
let loadingThread = '';
async function loadThread() {
  if (!snapshot || !selectedThread || !channel) return;
  const key = `${generation}:${selectedTask}:${selectedThread}`;
  if (loadingThread === key) return;
  loadingThread = key;
  try {
    const result = await request('thread', parameters());
    if (key === `${generation}:${selectedTask}:${selectedThread}`) { detail = result; renderDetail(); }
  } finally { if (loadingThread === key) loadingThread = ''; }
}
function renderDetail() {
  if (!detail) return;
  $('transcript').replaceChildren(...detail.entries.map(entry => {
    const article = node('article', undefined, `entry ${entry.kind}`);
    article.append(node('div', t(entry.kind), 'label'), node('pre', entry.text)); return article;
  }));
  $('transcript').append(node('p', t('history'), 'hint'));
  const permission = detail.permission;
  if (renderedPermission !== permission?.id) permissionReviewed = false;
  renderedPermission = permission?.id || '';
  $('permission').replaceChildren();
  if (permission) {
    const card = node('section', undefined, 'permission');
    card.append(node('p', t('permission'), 'eyebrow'), node('h3', permission.title));
    if (!permission.complete) card.append(node('p', t('incomplete')));
    if (permission.raw_input) card.append(node('pre', permission.raw_input));
    for (const diff of permission.diffs) {
      const section = node('details'); section.open = true;
      section.append(node('summary', diff.path), node('p', t('before')), node('pre', diff.old_text ?? ''), node('p', t('after')), node('pre', diff.new_text));
      card.append(section);
    }
    if (permission.complete) {
      const label = node('label'), checkbox = node('input'); checkbox.type = 'checkbox'; checkbox.checked = permissionReviewed;
      checkbox.onchange = () => { permissionReviewed = checkbox.checked; controls(); };
      label.append(checkbox, document.createTextNode(t('reviewed'))); card.append(label);
    }
    const buttons = node('div', undefined, 'choices');
    for (const option of permission.options) {
      const button = node('button', t(option.kind === 'allow' ? 'approve' : 'reject'));
      button.dataset.mutation = 'true'; button.dataset.kind = option.kind;
      button.onclick = () => mutate('permission_response', { permission_id: permission.id, option_id: option.id }).catch(showError);
      buttons.append(button);
    }
    card.append(buttons); $('permission').append(card);
  }
  const oldQuestion = $('question').firstElementChild;
  const selections = oldQuestion instanceof HTMLFormElement && oldQuestion.dataset.id === detail.question?.id ? questionSelections(oldQuestion, detail.question) : {};
  $('question').replaceChildren();
  if (detail.question) renderQuestion(detail.question, selections);
  else if (detail.question_pending) $('question').append(node('p', t('questionLocal'), 'question'));
  controls();
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
function renderQuestion(question, selections = {}) {
  const form = node('form', undefined, 'question');
  form.dataset.id = question.id;
  form.append(node('h3', t('question')), node('pre', question.message));
  for (const field of question.fields) {
    const label = node('label', field.title || field.name);
    const select = node('select'); select.name = field.name; select.required = true;
    const chosen = selections[field.name];
    if (field.multi) {
      // multiple では空欄の選択肢が「選べてしまう」ので置かない（required が最低 1 件を担保）。
      select.multiple = true;
      select.size = Math.min(field.choices.length, 5);
      const picked = Array.isArray(chosen) ? chosen : (chosen ? [chosen] : []);
      for (const choice of field.choices) {
        const option = node('option', choice.label); option.value = choice.value;
        option.selected = picked.includes(choice.value);
        select.append(option);
      }
    } else {
      const blank = node('option', '—'); blank.value = ''; select.append(blank);
      for (const choice of field.choices) { const option = node('option', choice.label); option.value = choice.value; select.append(option); }
      select.value = (Array.isArray(chosen) ? chosen[0] : chosen) || '';
    }
    label.append(select); form.append(label);
  }
  const submit = node('button', t('answer')); submit.type = 'submit'; submit.dataset.mutation = 'true';
  const decline = node('button', t('decline')); decline.type = 'button'; decline.dataset.mutation = 'true';
  decline.onclick = () => mutate('question_response', { question_id: question.id, selections: null }).catch(showError);
  form.append(submit, decline);
  form.onsubmit = event => { event.preventDefault(); mutate('question_response', { question_id: question.id, selections: questionSelections(form, question) }).catch(showError); };
  $('question').append(form);
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
$('hosts').onchange = () => { device = known.find(d => d.room === $('hosts').value); connect().catch(showError); };
$('projects').onchange = () => { selectedTask = $('projects').value; selectedThread = ''; detail = null; renderProjects(); };
$('reconnect').onclick = () => connect().catch(showError);
$('composer').onsubmit = event => {
  event.preventDefault(); const message = $('message').value, destination = draftKey;
  if (!message.trim()) return;
  mutate('send_message', { message }).then(() => {
    if (draftKey === destination && $('message').value === message) $('message').value = '';
    if (drafts.get(destination) === message) drafts.delete(destination);
  }).catch(showError);
};
$('interrupt').onclick = () => mutate('interrupt').catch(showError);
$('new-thread').onclick = () => {
  const agent = prompt(t('newAgent'), 'Claude Code');
  if (agent) mutate('new_thread', { agent }).then(result => { selectedThread = result.thread_id; detail = null; renderProjects(); }).catch(showError);
};
$('diff').onclick = async () => {
  try { const result = await request('get_diff', parameters()); $('diff-text').textContent = result.diff || t('noDiff'); $('diff-dialog').showModal(); }
  catch (error) { showError(error); }
};
$('close-diff').onclick = () => $('diff-dialog').close();
$('forget').onclick = async () => {
  if (!confirm(t('forgetConfirm'))) return;
  try {
    resetConnection(); await removeDevice(device.room); known = await devices(); device = known[0];
    snapshot = null; detail = null; refreshHosts(); await connect();
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
$('language').onchange = () => { setLanguage($('language').value); staticText(); renderProjects(); renderDetail(); };
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
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').catch(showError);
  navigator.serviceWorker.addEventListener('message', event => { if (event.data?.type === 'refresh') connect().catch(showError); });
}
staticText();
try {
  known = await devices(); device = known[0]; refreshHosts();
  if (initialFragment) await pairing(initialFragment); else if (device) await connect();
} catch (error) { showError(error); }
