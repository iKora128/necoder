// アカウント不要の入口では Content-Length を信用せず、読み込み中にバイト上限を掛ける。
export const MAX_ROOM_BODY = 2048;
interface Credentials { host: string; phone: string }
const rejected = (status: number, error: string) => Response.json({ error }, {
  status, headers: { 'Cache-Control': 'no-store' },
});

export async function readRoomCredentials(request: Request): Promise<Credentials | Response> {
  const reader = request.body?.getReader();
  if (!reader) return rejected(400, 'invalid_json');
  try {
    if (Number(request.headers.get('Content-Length')) > MAX_ROOM_BODY) {
      await reader.cancel();
      return rejected(413, 'too_large');
    }
    const bytes = new Uint8Array(MAX_ROOM_BODY);
    let length = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (length + value.byteLength > MAX_ROOM_BODY) {
        await reader.cancel();
        return rejected(413, 'too_large');
      }
      bytes.set(value, length);
      length += value.byteLength;
    }
    let value: unknown;
    try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true, ignoreBOM: false }).decode(bytes.subarray(0, length))); }
    catch { return rejected(400, 'invalid_json'); }
    if (!value || typeof value !== 'object' || Array.isArray(value)) return rejected(400, 'invalid_credentials');
    const credentials = value as Record<string, unknown>;
    const valid = (token: unknown): token is string => typeof token === 'string' && /^[A-Za-z0-9_-]{43}$/.test(token);
    if (!valid(credentials.host) || !valid(credentials.phone) || credentials.host === credentials.phone)
      return rejected(400, 'invalid_credentials');
    return { host: credentials.host, phone: credentials.phone };
  } catch {
    return rejected(400, 'invalid_body');
  } finally {
    reader.releaseLock();
  }
}
