import { DurableObject } from 'cloudflare:workers';

interface Env { ROOMS: DurableObjectNamespace<ControlRoom>; ASSETS: Fetcher; PROVISION_TOKEN: string; APP_ORIGIN: string; }
interface Room { hostHash: string; phoneHash: string; created: number; expires: number; }
interface Peer { role: 'host' | 'phone'; window: number; messages: number; bytes: number; }
const MAX_FRAME = 2_800_000;
const valid = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{43}$/.test(value);
const response = (status: number, error: string) => Response.json({ error }, { status, headers: { 'Cache-Control': 'no-store' } });
async function hash(value: string) {
  const bytes = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value)));
  return [...bytes].map(value => value.toString(16).padStart(2, '0')).join('');
}
async function equal(a: string, b: string) {
  // 固定長ダイジェストの全バイトを比較する。生の資格情報は DO に保存しない。
  const [one, two] = await Promise.all([hash(a), hash(b)]);
  let difference = 0;
  for (let i = 0; i < one.length; i++) difference |= one.charCodeAt(i) ^ two.charCodeAt(i);
  return difference === 0;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (!url.pathname.startsWith('/api/')) return env.ASSETS.fetch(request);
    const origin = request.headers.get('Origin');
    if (origin && origin !== env.APP_ORIGIN) return response(403, 'origin_rejected');
    if (url.pathname === '/api/health') return Response.json({ version: 1 }, { headers: { 'Cache-Control': 'no-store' } });
    const match = /^\/api\/rooms\/([A-Za-z0-9_-]{43})(\/ws)?$/.exec(url.pathname);
    if (!match) return response(404, 'not_found');
    if (request.method === 'POST') {
      if (!env.PROVISION_TOKEN || !await equal(request.headers.get('Authorization') ?? '', `Bearer ${env.PROVISION_TOKEN}`))
        return response(401, 'provisioning_required');
    }
    return env.ROOMS.getByName(match[1]).fetch(request);
  },
} satisfies ExportedHandler<Env>;

export class ControlRoom extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping', 'pong'));
  }
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    let room = await this.ctx.storage.get<Room>('room');
    if (request.method === 'POST' && !url.pathname.endsWith('/ws')) {
      if (room) return response(409, 'room_exists');
      if (Number(request.headers.get('Content-Length')) > 2048) return response(413, 'too_large');
      const body = await request.text();
      if (body.length > 2048) return response(413, 'too_large');
      let credentials: { host: string; phone: string };
      try { credentials = JSON.parse(body); } catch { return response(400, 'invalid_json'); }
      if (!valid(credentials.host) || !valid(credentials.phone) || credentials.host === credentials.phone) return response(400, 'invalid_credentials');
      room = { hostHash: await hash(credentials.host), phoneHash: await hash(credentials.phone), created: Date.now(), expires: Date.now() + 300_000 };
      await this.ctx.storage.put('room', room);
      await this.ctx.storage.setAlarm(room.expires);
      return Response.json({ created: true }, { status: 201, headers: { 'Cache-Control': 'no-store' } });
    }
    if (!room || room.expires <= Date.now()) return response(410, 'room_expired');
    if (request.method === 'DELETE' || request.method === 'PATCH') {
      if (!await this.authorized(request, room.hostHash)) return response(401, 'unauthorized');
      if (request.method === 'DELETE') {
        for (const peer of this.ctx.getWebSockets()) peer.close(4003, 'revoked');
        await this.ctx.storage.deleteAll();
      } else {
        room.expires = Date.now() + 30 * 86400_000;
        await this.ctx.storage.put('room', room);
        await this.ctx.storage.setAlarm(room.expires);
      }
      return Response.json({ ok: true });
    }
    if (request.method !== 'GET' || !url.pathname.endsWith('/ws') || request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') return response(400, 'websocket_required');
    // Browser WebSocket API は Authorization を設定できない。秘密は URL/log ではなく subprotocol へ。
    const protocols = (request.headers.get('Sec-WebSocket-Protocol') ?? '').split(',').map(v => v.trim());
    if (!protocols.includes('necoder-v1')) return response(400, 'protocol_required');
    const token = protocols.find(p => p.startsWith('auth.'))?.slice(5);
    if (!valid(token)) return response(401, 'unauthorized');
    const tokenHash = await hash(token);
    const role = tokenHash === room.hostHash ? 'host' : tokenHash === room.phoneHash ? 'phone' : null;
    if (!role) return response(401, 'unauthorized');
    // 同一端末の再接続は旧接続を閉じる。新しい接続では E2E の再認証が必須。
    for (const peer of this.ctx.getWebSockets(role)) peer.close(4001, 'replaced');
    const pair = new WebSocketPair();
    const client = pair[0], server = pair[1];
    this.ctx.acceptWebSocket(server, [role]);
    server.serializeAttachment({ role, window: Date.now(), messages: 0, bytes: 0 } satisfies Peer);
    const others = this.ctx.getWebSockets(role === 'host' ? 'phone' : 'host').filter(peer => peer.readyState === 1);
    for (const peer of others) peer.send(JSON.stringify({ type: 'peer', online: true }));
    server.send(JSON.stringify({ type: 'peer', online: others.length > 0 }));
    return new Response(null, { status: 101, webSocket: client, headers: { 'Sec-WebSocket-Protocol': 'necoder-v1' } });
  }
  async authorized(request: Request, expected: string) {
    const token = request.headers.get('Authorization')?.replace(/^Bearer /, '') ?? '';
    return valid(token) && await hash(token) === expected;
  }
  webSocketMessage(socket: WebSocket, message: string | ArrayBuffer) {
    const peer = socket.deserializeAttachment() as Peer;
    const size = typeof message === 'string' ? new TextEncoder().encode(message).length : message.byteLength;
    if (typeof message !== 'string' || size > MAX_FRAME) { socket.close(1009, 'frame_too_large'); return; }
    if (Date.now() - peer.window > 60_000) { peer.window = Date.now(); peer.messages = 0; peer.bytes = 0; }
    peer.messages++; peer.bytes += size;
    if (peer.messages > 1200 || peer.bytes > 24_000_000) { socket.close(1008, 'rate_limit'); return; }
    socket.serializeAttachment(peer);
    // 平文の payload は保存しない。接続状態以外を解釈せず、そのまま相手へ転送。
    if (socket.readyState !== 1) return;
    for (const target of this.ctx.getWebSockets(peer.role === 'host' ? 'phone' : 'host')) if (target.readyState === 1) target.send(message);
  }
  webSocketClose(socket: WebSocket, code: number, reason: string, wasClean: boolean) {
    const role = (socket.deserializeAttachment() as Peer).role;
    // 入れ替えで閉じた古い socket の close が新しい接続を offline にしない。
    const active = this.ctx.getWebSockets(role).filter(peer => peer !== socket && peer.readyState === 1);
    if (!active.length) for (const peer of this.ctx.getWebSockets(role === 'host' ? 'phone' : 'host')) if (peer.readyState === 1) peer.send(JSON.stringify({ type: 'peer', online: false }));
    socket.close(1000, 'closed');
  }
  webSocketError(socket: WebSocket) { socket.close(1011, 'connection_error'); }
  async alarm() {
    const room = await this.ctx.storage.get<Room>('room');
    if (room && room.expires > Date.now()) { await this.ctx.storage.setAlarm(room.expires); return; }
    for (const peer of this.ctx.getWebSockets()) peer.close(4003, 'expired');
    await this.ctx.storage.deleteAll();
  }
}
