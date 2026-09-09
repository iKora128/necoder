import WebSocket from 'ws';
import webpush from 'web-push';
import { accept, random, validSecret } from '../public/crypto.mjs';
import { ipc } from './ipc.mjs';
import { guiSocket, readState, writeState } from './storage.mjs';

export const ALLOWED = new Set(['snapshot', 'thread', 'get_diff', 'send_message', 'interrupt', 'permission_response', 'new_thread', 'question_response']);
const MUTATIONS = new Set(['send_message', 'interrupt', 'permission_response', 'new_thread', 'question_response']);
export function validateCommand(message, device, snapshot) {
  if (!message || message.type !== 'request' || !validSecret(message.id) || !ALLOWED.has(message.method)) throw new Error('invalid_request');
  if (!Number.isSafeInteger(message.expires_at) || message.expires_at <= Date.now() || message.expires_at > Date.now() + 60_000) throw new Error('command_expired');
  if (message.method === 'snapshot') return;
  if (!snapshot || message.params?.instance_id !== snapshot.instance_id) throw new Error('host_state_changed');
  if (!device.tasks.includes(message.params?.task_id)) throw new Error('project_not_shared');
}
export function validSubscription(subscription) {
  try {
    const url = new URL(subscription.endpoint);
    const allowed = url.hostname.endsWith('.push.apple.com') || url.hostname === 'fcm.googleapis.com'
      || url.hostname === 'updates.push.services.mozilla.com';
    return allowed && url.protocol === 'https:' && !url.port && !url.username && !url.password
      && typeof subscription.keys?.p256dh === 'string' && /^[A-Za-z0-9_-]{87}$/.test(subscription.keys.p256dh)
      && typeof subscription.keys.auth === 'string' && /^[A-Za-z0-9_-]{22}$/.test(subscription.keys.auth);
  } catch { return false; }
}

export class Bridge {
  constructor(config, devices, options = {}) {
    this.config = config; this.devices = devices;
    this.connections = new Map(); this.snapshot = null; this.stopped = false;
    this.ipc = options.ipc || ((method, params) => ipc(guiSocket, method, params));
    this.persist = options.persist || (() => writeState('devices', this.devices));
    this.persistReceipts = options.persistReceipts || (receipts => writeState('receipts', receipts));
    this.receipts = {}; this.polling = false;
  }
  async start() {
    this.receipts = await readState('receipts', {});
    for (const device of this.devices) if (!device.revoked) this.connect(device);
    await this.poll();
    this.timer = setInterval(() => this.poll().catch(error => console.error('Host poll:', error.message)), 1500);
    this.renewTimer = setInterval(() => this.renew().catch(error => console.error('Room renewal:', error.message)), 12 * 3600_000);
    await this.renew();
  }
  async renew() {
    for (const device of this.devices) if (device.paired && !device.revoked) {
      const response = await fetch(`${this.config.origin}/api/rooms/${device.room}`, { method: 'PATCH',
        headers: { Authorization: `Bearer ${device.hostToken}` }, signal: AbortSignal.timeout(10_000) }).catch(() => null);
      if (response?.status === 410) console.error('Room expired; pair this device again:', device.name);
    }
  }
  connect(device) {
    if (this.stopped || device.revoked || (!device.paired && device.expires <= Date.now())) return;
    const state = this.connections.get(device.room) || { attempt: 0 };
    clearTimeout(state.retry);
    const url = new URL(`/api/rooms/${device.room}/ws`, this.config.origin);
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    const socket = new WebSocket(url, ['necoder-v1', `auth.${device.hostToken}`], { maxPayload: 2_800_000, handshakeTimeout: 10_000, perMessageDeflate: false });
    Object.assign(state, { socket, channel: null, pairedChannel: false, outbound: Promise.resolve(), inbound: Promise.resolve(), lastPong: Date.now(), lastThread: '', selection: null });
    this.connections.set(device.room, state);
    socket.on('open', () => {
      state.attempt = 0;
      state.heartbeat = setInterval(() => {
        if (Date.now() - state.lastPong > 65_000) socket.terminate();
        else if (socket.readyState === WebSocket.OPEN) socket.send('ping');
      }, 25_000);
    });
    socket.on('message', bytes => {
      const raw = bytes.toString();
      if (raw === 'pong') { state.lastPong = Date.now(); return; }
      state.inbound = state.inbound.then(() => this.receive(device, state, JSON.parse(raw))).catch(error => {
        console.error('Remote connection rejected:', error.message);
        socket.close(1008, 'invalid_session');
      });
    });
    socket.on('error', () => {}); // close が再接続と状態表示を担当する。
    socket.on('close', () => {
      clearInterval(state.heartbeat); state.channel = null; state.pairedChannel = false;
      if (!this.stopped && !device.revoked) state.retry = setTimeout(() => this.connect(device),
        Math.min(30_000, 1000 * 2 ** Math.min(5, state.attempt++)) + Math.random() * 700);
    });
  }
  async receive(device, state, message) {
    if (device.revoked) throw new Error('device_revoked');
    if (message.type === 'peer') {
      // この通知はリレー生成なので認証済み online とはみなさない。
      state.channel = null; state.pairedChannel = false; state.selection = null;
      return;
    }
    if (message.type === 'hello') {
      if (!device.paired && device.expires <= Date.now()) throw new Error('pairing_expired');
      const result = await accept(device.secret, device.room, message);
      state.channel = result.channel; state.pairedChannel = false;
      state.socket.send(JSON.stringify(result.welcome));
      if (!device.paired) {
        // QR 秘密はこの成功で消費。復帰用の新しい鍵を暗号チャネル内だけで送る。
        device.secret = random(); device.paired = true; device.confirmed = false;
        await this.persist();
        await this.send(state, { type: 'paired', secret: device.secret, name: this.config.name, room: device.room });
        await this.renew();
      } else {
        await this.send(state, { type: 'ready', name: this.config.name });
      }
      return;
    }
    if (!state.channel) throw new Error('authentication_required');
    const value = await state.channel.open(message);
    if (value.type === 'confirm') {
      device.confirmed = true; state.pairedChannel = true; await this.persist();
      await this.sendSnapshot(device, state); return;
    }
    if (value.type === 'subscribe') {
      if (!device.confirmed || !validSubscription(value.subscription)) throw new Error('invalid_push_subscription');
      device.subscription = value.subscription; await this.persist();
      await this.send(state, { type: 'push_saved' }); return;
    }
    if (value.type === 'unsubscribe') {
      delete device.subscription; await this.persist();
      await this.send(state, { type: 'push_removed' }); return;
    }
    if (value.type === 'heartbeat') { await this.send(state, { type: 'heartbeat', now: Date.now() }); return; }
    if (!device.confirmed) throw new Error('pairing_confirmation_required');
    state.pairedChannel = true;
    let response;
    try { response = { type: 'response', id: value.id, ok: true, result: await this.execute(device, state, value) }; }
    catch (error) { response = { type: 'response', id: value?.id, ok: false, error: error.message }; }
    await this.send(state, response);
  }
  async execute(device, state, message) {
    if (device.revoked) throw new Error('device_revoked');
    validateCommand(message, device, this.snapshot);
    if (message.method === 'snapshot') {
      await this.sendSnapshot(device, state); return { online: !!this.snapshot };
    }
    const parameters = { ...message.params, expires_at: message.expires_at };
    const receiptKey = `${device.room}:${message.id}`;
    if (this.receipts[receiptKey]) {
      const previous = this.receipts[receiptKey];
      if (previous.status === 'done') return previous.result;
      throw new Error('outcome_unknown_check_history');
    }
    if (MUTATIONS.has(message.method)) {
      // IPC 送信前に記録する。クラッシュ後の自動再送で同じ命令を二重実行しない。
      this.receipts[receiptKey] = { status: 'pending', time: Date.now() };
      this.receipts = Object.fromEntries(Object.entries(this.receipts).filter(([, value]) => value.time > Date.now() - 86400_000));
      await this.persistReceipts(this.receipts);
    }
    if (device.revoked) throw new Error('device_revoked');
    const result = await this.ipc(`remote_${message.method}`, parameters);
    if (message.method === 'thread') {
      state.selection = parameters;
      state.lastThread = JSON.stringify(result);
    }
    if (MUTATIONS.has(message.method)) {
      this.receipts[receiptKey] = { status: 'done', result, time: Date.now() };
      await this.persistReceipts(this.receipts);
    }
    return result;
  }
  send(state, value) {
    const channel = state.channel;
    if (!channel || state.socket.readyState !== WebSocket.OPEN) return Promise.resolve();
    const send = state.outbound.then(async () => {
      if (state.channel !== channel) return;
      if (state.socket.bufferedAmount > 3_000_000) { state.socket.close(1008, 'slow_reader'); return; }
      const frame = await channel.seal(value);
      if (state.channel === channel && state.socket.readyState === WebSocket.OPEN) state.socket.send(JSON.stringify(frame));
    });
    state.outbound = send.catch(() => state.socket.close(1011, 'send_failed'));
    return send;
  }
  async sendSnapshot(device, state) {
    const snapshot = this.snapshot ? { ...this.snapshot, projects: this.snapshot.projects.filter(project => device.tasks.includes(project.id)) } : null;
    await this.send(state, { type: 'snapshot', online: !!snapshot, snapshot, name: this.config.name,
      vapid: this.config.vapid.publicKey, push_subscribed: !!device.subscription, now: Date.now() });
  }
  async poll() {
    if (this.polling || this.stopped) return;
    this.polling = true;
    try {
      let next = null;
      try { next = await this.ipc('remote_snapshot', {}); } catch { /* GUI 不在は通常状態。 */ }
      const changed = JSON.stringify(next) !== JSON.stringify(this.snapshot);
      this.snapshot = next;
      for (const device of this.devices) {
        if (device.revoked) continue;
        const state = this.connections.get(device.room);
        if (state?.channel && device.confirmed) {
          if (changed) await this.sendSnapshot(device, state);
          if (state.selection && next?.instance_id === state.selection.instance_id) {
            try {
              const detail = await this.ipc('remote_thread', state.selection);
              const serialized = JSON.stringify(detail);
              if (serialized !== state.lastThread) {
                state.lastThread = serialized;
                await this.send(state, { type: 'thread', task_id: state.selection.task_id, detail });
              }
            } catch { state.selection = null; }
          }
        }
        if (next && device.subscription) await this.maybeNotify(device, next);
      }
    } finally { this.polling = false; }
  }
  async maybeNotify(device, snapshot) {
    const pending = snapshot.projects.filter(project => device.tasks.includes(project.id))
      .flatMap(project => project.threads).filter(thread => thread.permission_id || thread.question_pending);
    const fingerprint = pending.map(thread => thread.permission_id || thread.question_id || thread.id + ':question').sort().join(',');
    if (fingerprint === device.lastNotification) return;
    device.lastNotification = fingerprint;
    await this.persist();
    if (!pending.length) return;
    try {
      // Push にコード・プロジェクト名・入力は含めない。送信先も既知の Push サービスへ限定。
      await webpush.sendNotification(device.subscription, JSON.stringify({ type: 'attention' }), {
        TTL: 300, timeout: 10_000, urgency: 'normal', vapidDetails: {
          subject: this.config.origin, publicKey: this.config.vapid.publicKey, privateKey: this.config.vapid.privateKey,
        },
      });
    } catch (error) {
      if (error.statusCode === 404 || error.statusCode === 410) { delete device.subscription; await this.persist(); }
      console.error('Push delivery failed:', error.statusCode || 'network');
    }
  }
  async pair(name, tasks) {
    if (!this.config.provisionToken) throw new Error('provision_token_missing');
    if (!this.snapshot) throw new Error('necoder_not_running_or_update_required');
    const selected = tasks?.length ? tasks : this.snapshot.projects.map(project => project.id);
    if (!selected.length || selected.some(id => !this.snapshot.projects.some(project => project.id === id))) throw new Error('open_a_project_first');
    if (this.devices.filter(device => !device.revoked && (device.paired || device.expires > Date.now())).length >= 8) throw new Error('device_limit_revoke_unused_devices');
    const device = { room: random(), hostToken: random(), phoneToken: random(), secret: random(),
      name: String(name || 'Phone').slice(0, 80), tasks: selected, paired: false, expires: Date.now() + 300_000 };
    const result = await fetch(`${this.config.origin}/api/rooms/${device.room}`, { method: 'POST',
      headers: { Authorization: `Bearer ${this.config.provisionToken}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ host: device.hostToken, phone: device.phoneToken }), signal: AbortSignal.timeout(10_000) });
    if (!result.ok) throw new Error(`room_create_${result.status}`);
    this.devices.push(device); await this.persist(); this.connect(device);
    const fragment = new URLSearchParams({ room: device.room, token: device.phoneToken, secret: device.secret, expires: String(device.expires) });
    return { url: `${this.config.origin}/#${fragment}`, expires: device.expires, tasks: selected };
  }
  async revoke(room) {
    const device = this.devices.find(device => device.room === room);
    if (!device) throw new Error('device_not_found');
    device.revoked = true; delete device.secret; delete device.subscription;
    await this.persist();
    const state = this.connections.get(room);
    if (state) { state.channel = null; state.pairedChannel = false; clearTimeout(state.retry); state.socket.close(4003, 'revoked'); }
    const result = await fetch(`${this.config.origin}/api/rooms/${room}`, { method: 'DELETE',
      headers: { Authorization: `Bearer ${device.hostToken}` }, signal: AbortSignal.timeout(10_000) }).catch(() => null);
    // ローカル失効はネット不通でも完了する。再起動してもこの端末は認証されない。
    return { revoked: true, relay_deleted: !!result?.ok };
  }
  stop() {
    this.stopped = true; clearInterval(this.timer); clearInterval(this.renewTimer);
    for (const state of this.connections.values()) { clearTimeout(state.retry); clearInterval(state.heartbeat); state.socket.close(1000, 'host_stopped'); }
  }
}
