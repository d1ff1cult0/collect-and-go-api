/*
 * De relay: een echte Chromium die namens Prijsspoor Collect&Go opvraagt.
 *
 * Overgenomen van de prijsbrowser van de VTK-website (infra/price-browser). Collect&Go
 * weigert een gewone aanvraag vanaf het IP van een server met HTTP 456 ("Colruyt | AntiBot").
 * Een browser die de site echt bezoekt, en eventueel via een residentieel IP gaat
 * (RELAY_PROXY, of de relay zelf thuis laten draaien), komt wel door.
 *
 * Hoe, in twee stappen:
 * 1. Een gewone HTTP-aanvraag vanaf het IP van de relay (of via RELAY_PROXY), precies zoals
 *    Prijsspoor ze rechtstreeks doet. Vanaf een residentieel IP komt die meestal gewoon door;
 *    dat is het eigenlijke werk van de relay.
 * 2. Wordt een pagina van de website (www) toch tegengehouden, dan gaat ze langs een echte
 *    browser: er blijft één Collect&Go-pagina open, en de opvraging is een `fetch` vanuit die
 *    pagina, met haar cookies, zonder iets te tekenen. Stuurt de botbeveiliging een
 *    wachtpagina, dan opent de relay de startpagina opnieuw en zit die uit zoals een
 *    bezoeker. Lukt ook dat niet, dan antwoordt de relay tien minuten lang meteen
 *    "tegengehouden" (456). De API gaat nooit langs de pagina: de gateway verwacht van een
 *    browser een challenge-handtekening, en die maken we bewust niet na.
 * Geen captcha's oplossen, geen vingerafdruk of challenge vervalsen.
 *
 * Wat dit bewust NIET is: een open proxy. De relay
 * - vraagt een gedeeld geheim (RELAY_SECRET, minstens 16 tekens);
 * - vraagt enkel https-adressen op van de hosts in RELAY_HOSTS (standaard collectandgo.be
 *   en zijn subdomeinen) en breekt een navigatie naar elders af;
 * - laadt geen afbeeldingen, video of lettertypes, en doet hoogstens twee opvragingen tegelijk;
 * - geeft enkel de body en de status terug, geen cookies of headers van Collect&Go.
 *
 * API:
 *   GET  /health
 *   POST /fetch   { "url": "...", "accept": "application/json", "apiKey": "..." }
 *        header   x-relay-secret: <RELAY_SECRET>
 *        antwoord de ruwe body; de status van Collect&Go staat in `x-relay-status`.
 *
 * CommonJS, zodat NODE_PATH werkt: `playwright` wordt naast het script geïnstalleerd.
 */

const http = require('node:http');
const { timingSafeEqual } = require('node:crypto');
const zlib = require('node:zlib');
const { chromium, request: playwrightRequest } = require('playwright');

const PORT = Number(process.env.PORT || 8787);
const HOST = process.env.RELAY_LISTEN || '0.0.0.0';
const SECRET = process.env.RELAY_SECRET || '';
const PROXY = process.env.RELAY_PROXY || '';
// Headless meldt zich als "HeadlessChrome" en houdt de botbeveiliging tegen; in de container
// draait Chromium daarom met een venster op een virtueel scherm (xvfb-run).
const HEADLESS = process.env.RELAY_HEADLESS === '1';
const CHROMIUM_ARGS = (process.env.RELAY_CHROMIUM_ARGS || '').split(' ').filter(Boolean);
// User-agent van de gewone aanvragen (stap 1), dezelfde als die van Prijsspoor zelf.
const USER_AGENT =
  process.env.RELAY_USER_AGENT ||
  'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36';
const ALLOWED = (process.env.RELAY_HOSTS || 'collectandgo.be')
  .split(',')
  .map((h) => h.trim().toLowerCase())
  .filter(Boolean);
const HOME = 'https://www.collectandgo.be/';
const MAX_PARALLEL = 2;
const NAV_TIMEOUT = 30_000;
const SESSION_MS = 20 * 60 * 1000;
const PAUSE_MS = 10 * 60 * 1000;
const MAX_BODY = 40 * 1024 * 1024;
const ANTIBOT = /antibot|even geduld|just a moment|checking your browser/i;

if (SECRET.length < 16) {
  console.error('RELAY_SECRET ontbreekt of is korter dan 16 tekens (maak er een met: openssl rand -hex 24)');
  process.exit(1);
}

function allowedHost(hostname) {
  const host = hostname.toLowerCase().replace(/^www\./, '');
  return ALLOWED.some((h) => host === h || host.endsWith(`.${h}`));
}

function sameSecret(given) {
  const a = Buffer.from(String(given || ''));
  const b = Buffer.from(SECRET);
  return a.length === b.length && timingSafeEqual(a, b);
}

// --- browser ------------------------------------------------------------------------------

let contextPromise = null;
function context() {
  contextPromise ??= (async () => {
    const browser = await chromium.launch({
      headless: HEADLESS,
      args: CHROMIUM_ARGS,
      ...(PROXY ? { proxy: { server: PROXY } } : {}),
    });
    browser.on('disconnected', () => {
      contextPromise = null;
      session = null;
    });
    const ctx = await browser.newContext({
      locale: 'nl-BE',
      timezoneId: 'Europe/Brussels',
      ...(PROXY ? { proxy: { server: PROXY } } : {}),
    });
    await ctx.route('**/*', (route) => {
      const request = route.request();
      if (['image', 'media', 'font'].includes(request.resourceType())) return route.abort();
      // De pagina zelf blijft bij Collect&Go; wat ze inlaadt (scripts, API's) mag verder.
      if (request.isNavigationRequest() && request.frame().parentFrame() === null) {
        let host = '';
        try {
          host = new URL(request.url()).hostname;
        } catch {
          return route.abort();
        }
        if (!allowedHost(host)) return route.abort();
      }
      return route.continue();
    });
    return ctx;
  })().catch((err) => {
    contextPromise = null;
    throw err;
  });
  return contextPromise;
}

function limiter(max) {
  const queue = [];
  const state = { running: 0 };
  state.run = async (job) => {
    if (state.running >= max) await new Promise((resolve) => queue.push(resolve));
    state.running += 1;
    try {
      return await job();
    } finally {
      state.running -= 1;
      queue.shift()?.();
    }
  };
  return state;
}
const jobs = limiter(MAX_PARALLEL);

// --- de open Collect&Go-pagina --------------------------------------------------------------

let session = null;
let opening = null;
let blockedUntil = 0;

/** Eén startpagina tegelijk openen, ook wanneer twee vragen samen binnenkomen. */
function openSession(force = false) {
  opening ??= openSessionNow(force).finally(() => {
    opening = null;
  });
  return opening;
}

async function openSessionNow(force) {
  const now = Date.now();
  if (!force && session?.failed && now - session.at < 60_000) return session;
  if (!force && session && !session.failed && now - session.at < SESSION_MS && !session.page.isClosed()) return session;
  if (session?.page) await session.page.close().catch(() => {});
  session = null;
  const page = await (await context()).newPage();
  const response = await page.goto(HOME, { waitUntil: 'domcontentloaded', timeout: NAV_TIMEOUT }).catch(() => null);
  // Een wachtpagina laat een echte browser na enkele seconden door; dat uitzitten zoals een bezoeker.
  if (ANTIBOT.test(await page.title().catch(() => '')))
    await page
      .waitForFunction((src) => !new RegExp(src, 'i').test(document.title), ANTIBOT.source, { timeout: 25_000 })
      .catch(() => {});
  const title = await page.title().catch(() => '');
  const ready = !ANTIBOT.test(title) && allowedHost(new URL(page.url()).hostname);
  if (!ready || !response || response.status() >= 500) {
    await page.close().catch(() => {});
    session = { failed: true, at: now, title, status: response ? response.status() : 0 };
    console.warn(`startpagina niet bereikt (status ${session.status}, titel "${title}")`);
    return session;
  }
  session = { page, at: now };
  return session;
}

async function fetchInPage(page, url, accept, apiKey) {
  return page.evaluate(
    async ({ target, accept, apiKey, max }) => {
      const headers = { accept };
      if (apiKey) headers['x-cg-apikey'] = apiKey;
      const res = await fetch(target, { credentials: 'include', headers });
      const buf = new Uint8Array(await res.arrayBuffer());
      if (buf.length > max) return { status: 413, finalUrl: res.url, contentType: '', body: '' };
      let bin = '';
      for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode.apply(null, buf.subarray(i, i + 0x8000));
      return { status: res.status, finalUrl: res.url, contentType: res.headers.get('content-type') || '', body: btoa(bin) };
    },
    { target: url, accept, apiKey, max: MAX_BODY }
  ).then((found) => ({ ...found, body: Buffer.from(found.body, 'base64') }));
}

/*
 * Stap 1: een gewone aanvraag, los van de browser (eigen cookies, geen browsercookies),
 * via dezelfde proxy. Een eigen request-context, want die van de browser stuurt de
 * user-agent en cookies van de browser mee.
 */
let plainPromise = null;
function plainContext() {
  plainPromise ??= playwrightRequest
    .newContext({
      userAgent: USER_AGENT,
      extraHTTPHeaders: { 'accept-language': 'nl-BE,nl;q=0.9' },
      ...(PROXY ? { proxy: { server: PROXY } } : {}),
    })
    .catch((err) => {
      plainPromise = null;
      throw err;
    });
  return plainPromise;
}

async function fetchPlain(url, accept, apiKey) {
  const headers = { accept, referer: HOME };
  if (apiKey) {
    headers['x-cg-apikey'] = apiKey;
    headers.origin = 'https://www.collectandgo.be';
  }
  const res = await (await plainContext()).get(url, { headers, timeout: 90_000, maxRedirects: 3 });
  const body = await res.body();
  if (body.length > MAX_BODY) return { status: 413, finalUrl: res.url(), contentType: '', body: Buffer.alloc(0) };
  return { status: res.status(), finalUrl: res.url(), contentType: res.headers()['content-type'] || '', body };
}

function titleOf(found) {
  if (!/html/i.test(found.contentType)) return '';
  const head = found.body.subarray(0, 64 * 1024).toString('utf8');
  return /<title[^>]*>([^<]*)<\/title>/i.exec(head)?.[1]?.trim() || '';
}

const isBlocked = (found) => [403, 429, 456].includes(found.status) || ANTIBOT.test(titleOf(found));

const BLOCKED = { status: 456, finalUrl: '', contentType: 'text/plain', body: Buffer.alloc(0), blocked: true };

async function relayFetch(url, accept, apiKey) {
  const plain = await fetchPlain(url, accept, apiKey);
  // De API gaat nooit langs de browser; enkel webpagina's die tegengehouden worden wel.
  if (!isBlocked(plain) || !/^www\./i.test(new URL(url).hostname)) return plain;

  if (Date.now() < blockedUntil) return BLOCKED;
  let s = await openSession();
  if (!s.failed) {
    // Een fout hier (netwerk, gesloten pagina) is geen blokkade: die gaat als 502 terug.
    const found = await fetchInPage(s.page, url, accept, apiKey).catch((err) => {
      s.at = 0;
      throw err;
    });
    if (!isBlocked(found)) return found;
  }
  // Tegengehouden: de startpagina opnieuw openen (zit een wachtpagina uit) en één keer opnieuw.
  s = await openSession(true);
  if (!s.failed) {
    const again = await fetchInPage(s.page, url, accept, apiKey).catch(() => null);
    if (again && !isBlocked(again)) return again;
  }
  blockedUntil = Date.now() + PAUSE_MS;
  console.warn(`Collect&Go houdt de relay tegen; ${PAUSE_MS / 60000} minuten pauze`);
  return BLOCKED;
}

// --- HTTP -----------------------------------------------------------------------------------

function sendJson(res, code, body) {
  const text = JSON.stringify(body);
  res.writeHead(code, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(text) });
  res.end(text);
}

http
  .createServer((req, res) => {
    if (req.method === 'GET' && req.url === '/health')
      return sendJson(res, 200, {
        ok: true,
        running: jobs.running,
        session: session ? (session.failed ? 'failed' : 'open') : 'none',
        blocked: Date.now() < blockedUntil,
        proxy: Boolean(PROXY),
      });
    if (req.method !== 'POST' || req.url !== '/fetch') return sendJson(res, 404, { error: 'NOT_FOUND' });
    if (!sameSecret(req.headers['x-relay-secret'])) return sendJson(res, 401, { error: 'UNAUTHORIZED' });
    let raw = '';
    req.on('data', (chunk) => {
      raw += chunk;
      if (raw.length > 8192) req.destroy();
    });
    req.on('end', () => {
      let url;
      let accept = 'application/json';
      let apiKey = '';
      try {
        const body = JSON.parse(raw);
        url = new URL(String(body.url));
        if (typeof body.accept === 'string') accept = body.accept.slice(0, 200);
        if (typeof body.apiKey === 'string') apiKey = body.apiKey.slice(0, 100);
      } catch {
        return sendJson(res, 400, { error: 'BAD_REQUEST' });
      }
      if (url.protocol !== 'https:' || !allowedHost(url.hostname)) return sendJson(res, 400, { error: 'HOST_NOT_ALLOWED' });
      const started = Date.now();
      jobs
        .run(() => relayFetch(url.toString(), accept, apiKey))
        .then((found) => {
          const raw = found.body || Buffer.alloc(0);
          // Gecomprimeerd terug: draait de relay thuis, dan scheelt dat ruim 90% upload.
          const gzip = raw.length > 1024 && /\bgzip\b/.test(String(req.headers['accept-encoding'] || ''));
          const bytes = gzip ? zlib.gzipSync(raw, { level: 6 }) : raw;
          res.writeHead(200, {
            'content-type': found.contentType || 'application/octet-stream',
            'content-length': bytes.length,
            ...(gzip ? { 'content-encoding': 'gzip' } : {}),
            'x-relay-status': String(found.status),
            'x-relay-final-url': encodeURI(found.finalUrl || ''),
            ...(found.blocked ? { 'x-relay-blocked': '1' } : {}),
          });
          res.end(bytes);
          console.log(`${found.status} ${url.hostname}${url.pathname.slice(0, 80)} ${raw.length}B${gzip ? ` (${bytes.length}B gzip)` : ''} ${Date.now() - started}ms`);
        })
        .catch((err) => {
          const timeout = /timeout/i.test(String(err?.name)) || /timeout/i.test(String(err?.message));
          sendJson(res, 502, { error: timeout ? 'TIMEOUT' : 'BROWSER_ERROR' });
          if (!timeout) console.error('relay mislukt:', String(err?.message).slice(0, 300));
        });
    });
  })
  .listen(PORT, HOST, () =>
    console.log(`relay luistert op ${HOST}:${PORT}, hosts: ${ALLOWED.join(', ')}${PROXY ? ', via proxy' : ''}`)
  );
