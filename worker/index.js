// necoder.com — 静的アセット配信 + トップでの言語振り分け。
//
// 走るのは `/` だけ（wrangler.jsonc の assets.run_worker_first）。ほかのパスは
// Worker を通らずそのまま配信される。
//
// 振り分けの規則:
//   1. `?lang=ja|en` … 明示選択。cookie に憶えて、クエリ無しの正しい URL へ送る
//   2. cookie があればそれに従う（= 一度選んだら以後は言語判定しない）
//   3. 無ければ Accept-Language。日本語が最上位なら ja、そうでなければ en
// 日本語が正（`/` が canonical / x-default）で、英語は `/en/`。`/en/` は
// 振り分けの対象外＝共有されたリンクは常にその言語で開く。

const JA_PATH = '/';
const EN_PATH = '/en/';
const COOKIE_NAME = 'necoder_lang';
const COOKIE_MAX_AGE = 60 * 60 * 24 * 365;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    const explicit = url.searchParams.get('lang');
    if (explicit === 'ja' || explicit === 'en') {
      return languageRedirect(url, explicit === 'en' ? EN_PATH : JA_PATH, explicit);
    }

    const preferred = cookieLanguage(request) ?? headerLanguage(request);
    if (preferred === 'en') {
      return languageRedirect(url, EN_PATH, null);
    }

    // 日本語（既定）はそのまま配る。判定に使った入力を Vary に出しておく。
    const response = await env.ASSETS.fetch(request);
    const served = new Response(response.body, response);
    served.headers.append('Vary', 'Accept-Language, Cookie');
    return served;
  },
};

/** クエリを落として `pathname` へ 302。`remember` があれば選択を cookie に焼く。 */
function languageRedirect(url, pathname, remember) {
  const target = new URL(url);
  target.pathname = pathname;
  target.searchParams.delete('lang');

  const headers = new Headers({
    Location: target.toString(),
    // 人によって行き先が変わるので、共有キャッシュに載せない。
    'Cache-Control': 'no-store',
    Vary: 'Accept-Language, Cookie',
  });
  if (remember) {
    headers.append(
      'Set-Cookie',
      `${COOKIE_NAME}=${remember}; Path=/; Max-Age=${COOKIE_MAX_AGE}; SameSite=Lax`,
    );
  }
  return new Response(null, { status: 302, headers });
}

/** 明示選択の記憶。壊れた値は無視して言語判定に戻す。 */
function cookieLanguage(request) {
  const cookies = request.headers.get('Cookie') ?? '';
  const found = cookies.split(';').find((part) => part.trim().startsWith(`${COOKIE_NAME}=`));
  const value = found?.split('=')[1]?.trim();
  return value === 'ja' || value === 'en' ? value : null;
}

/**
 * 日本語より強く望まれている言語がある時だけ en。それ以外は既定の ja。
 *
 * 「ja が含まれるか」ではなく「一番強く望まれているか」で見る
 * （`en-US,en;q=0.9,ja;q=0.8` は英語話者なので en が正しい）。
 *
 * ヘッダが無い時に ja へ倒すのは意図的で、クローラは Accept-Language を送らないことが多く、
 * ここで en へ 302 すると **canonical の日本語ページが一度も読まれない**まま終わるため。
 * 実ブラウザは必ず送るので、英語話者の体験は変わらない。
 */
function headerLanguage(request) {
  const header = request.headers.get('Accept-Language');
  if (!header) return 'ja';

  let bestJa = 0;
  let bestOther = 0;
  for (const part of header.split(',')) {
    const [tag, ...parameters] = part.trim().split(';');
    if (!tag) continue;
    const quality = Number(
      parameters.find((parameter) => parameter.trim().startsWith('q='))?.split('=')[1] ?? 1,
    );
    const weight = Number.isFinite(quality) ? quality : 0;
    if (tag.toLowerCase().startsWith('ja')) bestJa = Math.max(bestJa, weight);
    else if (tag !== '*') bestOther = Math.max(bestOther, weight);
  }
  return bestOther > bestJa ? 'en' : 'ja';
}
