#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Playwright-over-CDP experiment / smoke test against QtWebEngine's DevTools port.
//
//   node tools/cdp-probe/playwright-attach.mjs [endpoint] [--out FILE]
//
// `endpoint` is anything Playwright's `chromium.connectOverCDP` accepts:
//   - http://127.0.0.1:9333                       (needs the mitm proxy: Qt rejects
//                                                  Browser.setDownloadBehavior; see README)
//   - ws://127.0.0.1:9333/devtools/browser/<id>
//   - ws://127.0.0.1:9333/devtools/page/<id>
//   - http://127.0.0.1:9222                       (direct against Qt: fails, by design)
// Default: http://127.0.0.1:9222.
//
// Playwright is NOT vendored; install it once into a scratch dir (see README):
//   PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm i --prefix /tmp/cdp-playwright playwright
// Resolution order: $PLAYWRIGHT_MODULE, the script's own node_modules,
// /tmp/cdp-playwright/node_modules.
//
// When attached, it runs a fixed step list (title, evaluate, Tauri invoke,
// console, pageerror, click, screenshot, cookies, viewport, goto, newPage) and
// records every step's outcome — successes with values, failures with the exact
// Playwright/CDP error. Exit code: 0 = attached, 1 = attach or a step failed,
// 2 = playwright missing / bad arguments.

import { writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const CANDIDATES = [
  process.env.PLAYWRIGHT_MODULE,
  'playwright',
  '/tmp/cdp-playwright/node_modules/playwright/index.js',
].filter(Boolean);

async function loadPlaywright() {
  const failures = [];
  for (const c of CANDIDATES) {
    try {
      const spec = c === 'playwright' ? c : pathToFileURL(c).href;
      const mod = await import(spec);
      return mod.default ?? mod;
    } catch (e) {
      if (e?.code === 'ERR_MODULE_NOT_FOUND' || e?.code === 'MODULE_NOT_FOUND') {
        failures.push(`${c}: not found`);
        continue;
      }
      failures.push(`${c}: ${e.message}`);
    }
  }
  console.error(`playwright-attach: cannot load playwright.\nTried:\n  ${failures.join('\n  ')}\nInstall it with:\n  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm i --prefix /tmp/cdp-playwright playwright`);
  process.exit(2);
}

const waitFor = (emitter, event, timeoutMs) =>
  new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      emitter.off(event, onEvent);
      reject(new Error(`no '${event}' event within ${timeoutMs} ms`));
    }, timeoutMs);
    const onEvent = (arg) => {
      clearTimeout(timer);
      resolve(arg);
    };
    emitter.once(event, onEvent);
  });

async function main() {
  const argv = process.argv.slice(2);
  let out = null;
  const positional = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--out') out = argv[++i];
    else positional.push(argv[i]);
  }
  const endpoint = positional[0] ?? 'http://127.0.0.1:9222';

  const pw = await loadPlaywright();
  const result = {
    endpoint,
    playwrightVersion: pw.version ?? null,
    startedAt: new Date().toISOString(),
    steps: [],
  };
  const steps = result.steps;
  const runStep = async (name, fn, { critical = false } = {}) => {
    const t0 = Date.now();
    try {
      const value = await fn();
      steps.push({ name, ok: true, ms: Date.now() - t0, value });
      console.log(`  ok    ${name}: ${typeof value === 'string' ? value : JSON.stringify(value)}`);
      return value;
    } catch (e) {
      const message = e?.message ?? String(e);
      steps.push({ name, ok: false, ms: Date.now() - t0, error: message, ...(e?.name && e.name !== 'Error' ? { errorName: e.name } : {}) });
      console.log(`  FAIL  ${name}: ${String(message).split('\n')[0]}`);
      if (critical) throw e;
      return undefined;
    }
  };

  const started = Date.now();
  let browser;
  try {
    console.log(`connectOverCDP(${endpoint})`);
    browser = await pw.chromium.connectOverCDP(endpoint, { timeout: 15_000 });
    result.attached = true;
    console.log('  ok    attach');
    await runStep('browser.version()', () => browser.version());
    await runStep('browser.contexts().length', () => browser.contexts().length);
    const context = browser.contexts()[0] ?? (await browser.newContext());
    const page = context.pages()[0] ?? (await context.newPage());
    result.pages = context.pages().length;

    await runStep('page.title()', () => page.title());
    await runStep('page.url()', () => page.url());
    await runStep("page.evaluate('1+1')", () => page.evaluate(() => 1 + 1));
    await runStep('page.evaluate(invoke greet)', () => page.evaluate(() => window.__TAURI_INTERNALS__.invoke('greet', { name: 'playwright' })));
    await runStep('page.evaluate(invoke report)', () => page.evaluate(() => window.__TAURI_INTERNALS__.invoke('report', { text: 'cdp-probe: playwright evaluate reached Rust' })));

    await runStep('console message reaches page.on("console")', async () => {
      const waiting = waitFor(page, 'console', 5000);
      await page.evaluate(() => console.log('playwright-console-probe'));
      const msg = await waiting;
      return `${msg.type()}: ${msg.text()}`;
    });
    await runStep('uncaught error reaches page.on("pageerror")', async () => {
      const waiting = waitFor(page, 'pageerror', 5000);
      await page.evaluate(() => setTimeout(() => { throw new Error('playwright-pageerror-probe'); }, 0));
      return (await waiting).message;
    });
    await runStep("page.click('#summary')", () => page.click('#summary'));
    await runStep('page.screenshot()', async () => {
      const buf = await page.screenshot({ timeout: 15_000 });
      return `${buf.length} bytes, png=${buf.subarray(0, 4).toString('hex') === '89504e47'}`;
    });
    await runStep('page.$eval("#summary", el => el.textContent)', () => page.$eval('#summary', (el) => el.textContent));
    await runStep('context.cookies() (before add)', async () => JSON.stringify((await context.cookies()).map((c) => c.name)));
    await runStep('context.addCookies([pw_probe])', () => context.addCookies([{ name: 'pw_probe', value: '1', url: 'http://localhost/' }]));
    await runStep('context.cookies() (after add)', async () => JSON.stringify((await context.cookies()).map((c) => c.name)));
    await runStep('context.clearCookies()', () => context.clearCookies());
    await runStep('context.cookies() (after clear)', async () => JSON.stringify((await context.cookies()).map((c) => c.name)));
    await runStep('page.setViewportSize({width:800,height:600})', () => page.setViewportSize({ width: 800, height: 600 }));
    await runStep('page.viewportSize()', () => JSON.stringify(page.viewportSize()));
    await runStep('network: page.on("request") during goto', async () => {
      const urls = [];
      const listener = (r) => urls.push(r.url());
      page.on('request', listener);
      const resp = await page.goto('tauri://localhost/', { timeout: 15_000 });
      await page.waitForTimeout(300);
      page.off('request', listener);
      return `goto status=${resp ? resp.status() : 'none'} requests=${urls.length} first=${urls[0] ?? 'none'}`;
    });
    await runStep('page.evaluate(document.title) after goto', () => page.evaluate(() => document.title));
    await runStep('context.newPage()', async () => {
      const p2 = await context.newPage();
      return `new page url=${p2.url()}`;
    });
    await runStep('browser.close()', async () => {
      await browser.close();
      return 'closed';
    });
    browser = null;
  } catch (e) {
    result.attached = result.attached ?? false;
    result.error = { name: e?.name ?? 'Error', message: e?.message ?? String(e) };
  }
  result.durationMs = Date.now() - started;
  result.finishedAt = new Date().toISOString();
  // Steps that are known to be unsupported on QtWebEngine (measured; see
  // docs/cdp-support-matrix.md). They are recorded as failures but don't make
  // the tool exit non-zero — `KNOWN_UNSUPPORTED=0` disables this.
  const knownUnsupported = new Set(['context.newPage()']);
  const tolerate = process.env.KNOWN_UNSUPPORTED !== '0';
  for (const s of steps) if (!s.ok && knownUnsupported.has(s.name)) s.knownUnsupported = true;
  const failed = steps.filter((s) => !s.ok && !(tolerate && s.knownUnsupported));
  console.log(`\nattached=${result.attached} steps ok=${steps.filter((s) => s.ok).length}/${steps.length} failed=${failed.length}${failed.length ? ` (${failed.map((s) => s.name).join(', ')})` : ''}`);
  const text = `${JSON.stringify(result, null, 2)}\n`;
  if (out) writeFileSync(out, text);
  console.log(text.length > 2000 && !out ? '(full JSON omitted; pass --out FILE to save)' : text);
  process.exit(result.attached && failed.length === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(`playwright-attach: unexpected failure: ${e.stack || e}`);
  process.exit(2);
});
