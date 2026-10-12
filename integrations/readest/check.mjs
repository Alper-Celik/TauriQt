#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Readest-on-Qt smoke, over QtWebEngine's DevTools protocol.
//
//   node integrations/readest/check.mjs [--port 9222] [--timeout-ms 5000]
//                                      [--url-substring localhost:3000]
//
// Start the app first: integrations/readest/run.sh opens CDP on 9222.
//
// The last check is the interesting one: `get_environment_variable` is a real
// Readest command, so a value coming back proves the whole path — page ->
// QtWebEngine custom scheme -> this runtime's scheme handler -> tauri IPC ->
// Rust -> back — which on this runtime means the header-carried payload
// (INVOKE_SYSTEM_SCRIPT) did its job. On a build without that wiring the call
// never reaches Rust.
//
// Exit codes: 0 all checks passed, 1 a check failed, 2 no CDP endpoint.

const args = { port: 9222, timeoutMs: 5000, urlSubstring: 'localhost:3000' };
for (let i = 2; i < process.argv.length; i += 1) {
  const [key, value] = [process.argv[i], process.argv[i + 1]];
  if (key === '--port') { args.port = Number(value); i += 1; }
  else if (key === '--timeout-ms') { args.timeoutMs = Number(value); i += 1; }
  else if (key === '--url-substring') { args.urlSubstring = value; i += 1; }
  else if (key === '--help') {
    console.log('usage: check.mjs [--port N] [--timeout-ms N] [--url-substring S]');
    process.exit(0);
  }
}

const base = `http://127.0.0.1:${args.port}`;

async function httpJson(path) {
  const res = await fetch(`${base}${path}`);
  if (!res.ok) throw new Error(`${path} -> ${res.status}`);
  return res.json();
}

const results = [];
function report(name, ok, detail) {
  results.push(ok);
  const text = detail === undefined ? '' : ` (${String(detail).replace(/\s+/g, ' ').slice(0, 120)})`;
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${text}`);
}

class Cdp {
  constructor(ws) { this.ws = ws; this.id = 0; this.pending = new Map(); }
  static async connect(url) {
    const ws = new WebSocket(url);
    // Bound the handshake too: a target that accepts TCP but never completes the
    // upgrade used to hang this script (and anything waiting on it) forever.
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        try { ws.close(); } catch { /* already gone */ }
        reject(new Error(`websocket connect timed out after ${args.timeoutMs} ms`));
      }, args.timeoutMs);
      const done = (fn) => (value) => { clearTimeout(timer); fn(value); };
      ws.addEventListener('open', done(resolve), { once: true });
      ws.addEventListener('error', done((e) => reject(new Error(`websocket: ${e.message ?? 'error'}`))), { once: true });
    });
    const client = new Cdp(ws);
    ws.addEventListener('message', (event) => {
      const msg = JSON.parse(event.data);
      const slot = client.pending.get(msg.id);
      if (slot) { client.pending.delete(msg.id); slot(msg); }
    });
    return client;
  }
  send(method, params = {}) {
    const id = (this.id += 1);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`${method} timed out`)); }, args.timeoutMs);
      this.pending.set(id, (msg) => {
        clearTimeout(timer);
        if (msg.error) reject(new Error(`${method}: ${msg.error.message}`));
        else resolve(msg.result);
      });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  /// Evaluate repeatedly until `predicate` accepts the value. A real app is an
  /// SPA: it navigates (loading skeleton -> route -> content), so a single
  /// sample can land in an empty DOM and mean nothing.
  async evalUntil(expression, predicate, timeoutMs = 10000, stepMs = 250, awaitPromise = false) {
    const deadline = Date.now() + timeoutMs;
    let last;
    do {
      last = await this.eval(expression, awaitPromise);
      if (predicate(last)) return last;
      await new Promise((done) => setTimeout(done, stepMs));
    } while (Date.now() < deadline);
    return last;
  }
  async eval(expression, awaitPromise = false) {
    const out = await this.send('Runtime.evaluate', { expression, awaitPromise, returnByValue: true });
    if (out.exceptionDetails) {
      throw new Error(out.exceptionDetails.exception?.description ?? out.exceptionDetails.text);
    }
    return out.result?.value;
  }
  close() { this.ws.close(); }
}

let version;
try {
  version = await httpJson('/json/version');
  report('CDP endpoint reachable', true, version['Browser'] ?? version['User-Agent'] ?? '');
} catch (err) {
  console.error(`  no CDP endpoint on ${base}: ${err.message}`);
  console.error('  is the app running? integrations/readest/run.sh opens it');
  process.exit(2);
}

const targets = await httpJson('/json/list');
const pages = targets.filter((t) => t.type === 'page');
const page = pages.find((t) => (t.url ?? '').includes(args.urlSubstring));
if (!page) {
  // Do not silently probe whatever target happens to be first: another browser
  // (or an app instance that has not navigated yet) can own the port.
  report(
    `a page whose url contains "${args.urlSubstring}"`,
    false,
    `${pages.length} page target(s): ${pages.map((t) => t.url || '(none)').join(' ')}`,
  );
  process.exit(1);
}
report('page target found', true, page.url);

const cdp = await Cdp.connect(page.webSocketDebuggerUrl);
try {
  const title = await cdp.eval('document.title');
  report('page has a title', Boolean(title), title);

  const internals = await cdp.eval('typeof window.__TAURI_INTERNALS__');
  report('window.__TAURI_INTERNALS__ present', internals === 'object', internals);

  const body = await cdp.evalUntil(
    'document.body ? document.body.innerText.slice(0, 80) : ""',
    (text) => Boolean(text && text.trim()),
  );
  if (body && body.trim()) {
    report('page rendered content', true, body);
  } else {
    // No text yet: fall back to the structural claim, which is still a real one.
    const children = await cdp.eval('document.body ? document.body.childElementCount : 0');
    report('page rendered content', children > 0, `childElementCount=${children}`);
  }

  const home = await cdp.eval(
    "window.__TAURI_INTERNALS__.invoke('get_environment_variable', { name: 'HOME' })",
    true,
  );
  report('invoke() round-trip reaches Rust', typeof home === 'string' && home.length > 0, home);
} catch (err) {
  report('checks ran', false, err.message);
} finally {
  cdp.close();
}

const failed = results.filter((ok) => !ok).length;
console.log(failed === 0 ? '==> all checks passed' : `==> ${failed} check(s) failed`);
process.exit(failed === 0 ? 0 : 1);
