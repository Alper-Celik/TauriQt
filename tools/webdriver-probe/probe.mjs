#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Drives ChromeDriver over its plain HTTP endpoint (no npm deps) against a
// running TauriQt app whose QtWebEngine DevTools port is open, and records
// every step — including the verbatim error bodies — into results.json.
//
// Usage:
//   node probe.mjs [--cdp=127.0.0.1:9223] [--port=4444] [--out=probe-out]
//                  [--chromedriver=/path/to/chromedriver] [--keep-driver]
//
// Exit code 0 when the script itself ran to completion (partial WebDriver
// failures are data, not script errors); 2 when the CDP endpoint or
// chromedriver could not be reached at all.

import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { startVersionFixup } from "./version-fixup.mjs";

const here = dirname(fileURLToPath(import.meta.url));

// ---------------------------------------------------------------- arguments
const args = new Map(
  process.argv.slice(2).map((arg) => {
    const [key, value = ""] = arg.replace(/^--/, "").split("=");
    return [key, value];
  }),
);
const CDP = args.get("cdp") || "127.0.0.1:9223";
const DRIVER_PORT = Number(args.get("port") || 4444);
const DRIVER_OUT = args.get("out") || join(here, "probe-out");
const CHROMEDRIVER =
  args.get("chromedriver") ||
  process.env.CHROMEDRIVER ||
  "/nix/store/f4higzcmy8c4s41vb8xvj4h4r9bdbcd9-chromedriver-unwrapped-155.0.8059.39/bin/chromedriver";
const KEEP_DRIVER = args.get("keep-driver") !== undefined;
// `--fixup=Chrome/140.0.0.0` starts the /json/version rewrite proxy and points
// ChromeDriver at it; without it ChromeDriver talks to Qt's port directly.
const FIXUP_VERSION = args.get("fixup") || "";
const FIXUP_PORT = Number(args.get("fixup-port") || 9224);
const CDP_BASE = `http://${CDP}`;
const DRIVER_BASE = `http://127.0.0.1:${DRIVER_PORT}`;
let DEBUGGER_ADDRESS = CDP;

mkdirSync(DRIVER_OUT, { recursive: true });
const logPath = join(DRIVER_OUT, "chromedriver-verbose.log");

// ------------------------------------------------------------- tiny helpers
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function http(method, url, body) {
  const init = { method };
  if (body !== undefined) {
    init.headers = { "content-type": "application/json; charset=utf-8" };
    init.body = JSON.stringify(body);
  }
  const response = await fetch(url, init);
  const text = await response.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = undefined;
  }
  return { status: response.status, text, json };
}

function log(...parts) {
  console.log(...parts);
}

// ------------------------------------------------------------------ records
const results = {
  startedAt: new Date().toISOString(),
  cdpAddress: CDP,
  chromedriver: { path: CHROMEDRIVER, port: DRIVER_PORT, version: null },
  cdp: { version: null, list: null },
  session: { request: null, response: null },
  steps: [],
  screenshot: null,
  verdict: null,
  cdpCommands: null,
  cdpFailures: null,
  driverRefusals: null,
};

function record(name, detail) {
  results.steps.push({ name, at: new Date().toISOString(), ...detail });
}

// ------------------------------------------------------------------- 1. CDP
async function probeCdp() {
  const version = await http("GET", `${CDP_BASE}/json/version`);
  results.cdp.version = { status: version.status, body: version.json ?? version.text };
  log(`GET ${CDP_BASE}/json/version -> ${version.status}`);
  log(version.text.trim());

  const list = await http("GET", `${CDP_BASE}/json/list`);
  results.cdp.list = { status: list.status, body: list.json ?? list.text };
  log(`\nGET ${CDP_BASE}/json/list -> ${list.status}`);
  log(list.text.trim());
  return version.status === 200;
}

// ------------------------------------------------- 2. chromedriver lifecycle
function startChromedriver() {
  const versionLine = spawnSync(CHROMEDRIVER, ["--version"], {
    env: { ...process.env, LD_LIBRARY_PATH: "" },
    encoding: "utf8",
  });
  results.chromedriver.version = (versionLine.stdout || "").trim();
  const match = results.chromedriver.version.match(/ChromeDriver (\d+)\.([\d.]+)/);
  results.chromedriver.major = match ? Number(match[1]) : null;
  log(`\nchromedriver --version: ${results.chromedriver.version}`);
  const child = spawn(
    CHROMEDRIVER,
    [`--port=${DRIVER_PORT}`, "--verbose", `--log-path=${logPath}`],
    {
      // The harness environment carries a LD_LIBRARY_PATH that breaks Nix
      // binaries; chromedriver is self-contained.
      env: { ...process.env, LD_LIBRARY_PATH: "" },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => (stdout += chunk));
  child.stderr.on("data", (chunk) => (stderr += chunk));
  child.on("exit", (code) => {
    results.chromedriver.exitCode = code;
    results.chromedriver.stderrTail = stderr.split("\n").slice(-20).join("\n");
  });
  return { child, getStdout: () => stdout, getStderr: () => stderr };
}

async function waitForDriver() {
  for (let attempt = 0; attempt < 60; attempt += 1) {
    try {
      const status = await http("GET", `${DRIVER_BASE}/status`);
      if (status.status === 200) return status;
    } catch {
      /* not up yet */
    }
    await sleep(250);
  }
  throw new Error(`chromedriver did not answer on ${DRIVER_BASE}/status`);
}

// ---------------------------------------------------- 4. W3C command battery
function elementId(body) {
  const key = "element-6066-11e4-a52e-4f735466cecf";
  return body?.value?.[key] ?? null;
}

async function driveStep(name, method, path, body, opts = {}) {
  const url = `${DRIVER_BASE}/session/${results.session.id}${path}`;
  const started = Date.now();
  let reply;
  try {
    reply = await http(method, url, body);
  } catch (err) {
    record(name, { request: { method, path, body }, transportError: String(err) });
    return { ok: false, error: String(err) };
  }
  const elapsed = Date.now() - started;
  const bodyText = reply.text.length > 2000 ? `${reply.text.slice(0, 2000)}…` : reply.text;
  const isError = reply.status >= 400 || (reply.json && reply.json.value?.error);
  const errorMessage =
    (reply.json && (reply.json.value?.message || reply.json.message)) || undefined;
  record(name, {
    request: { method, path, body: body ?? null },
    status: reply.status,
    elapsedMs: elapsed,
    body: bodyText,
    ok: !isError,
    error: errorMessage,
  });
  log(`  ${isError ? "FAIL" : "ok  "} ${name} [${method} ${path}] -> ${reply.status}${errorMessage ? ` ${errorMessage}` : ""}`);
  return { ok: !isError, status: reply.status, json: reply.json, text: reply.text, error: errorMessage };
}

async function driveCommands() {
  // Navigation. `tauri://localhost` is the custom scheme the runtime serves.
  await driveStep("url.get.initial", "GET", "/url");
  await driveStep("url.set.tauri", "POST", "/url", { url: "tauri://localhost" }, { navigate: true });
  await driveStep("url.get.after", "GET", "/url");
  await driveStep("title", "GET", "/title");
  await driveStep("source", "GET", "/source");

  await driveStep("execute/sync.title", "POST", "/execute/sync", {
    script: "return document.title;",
    args: [],
  });
  await driveStep("execute/sync.invoke.greet", "POST", "/execute/sync", {
    script:
      "return window.__TAURI_INTERNALS__.invoke('greet', { name: 'wd-sync' });",
    args: [],
  });
  await driveStep("execute/async.invoke.greet", "POST", "/execute/async", {
    script:
      "const done = arguments[arguments.length - 1];" +
      "window.__TAURI_INTERNALS__.invoke('greet', { name: 'wd-async' })" +
      ".then((value) => done(value), (err) => done('ERR: ' + err));",
    args: [],
  });

  // An input we control, so `element/{id}/value` has a form control to read.
  await driveStep("execute/sync.make-input", "POST", "/execute/sync", {
    script:
      "const input = document.createElement('input');" +
      "input.id = 'wd-probe-input'; input.value = 'wd-probe-value';" +
      "document.body.appendChild(input); return 'created';",
    args: [],
  });

  // Does a WebDriver click reach the page as a real input event? Install an
  // observable side effect first (and record event.isTrusted), then click.
  await driveStep("execute/sync.install-click-probe", "POST", "/execute/sync", {
    script:
      "const button = document.createElement('button');" +
      "button.id = 'wd-probe-button'; button.textContent = 'click me';" +
      "button.onclick = (event) => { window.__wd_click = event.isTrusted ? 'trusted' : 'untrusted'; };" +
      "document.body.appendChild(button); return 'installed';",
    args: [],
  });
  const findButton = await driveStep("element.find.button", "POST", "/element", {
    using: "css selector",
    value: "#wd-probe-button",
  });
  const buttonId = findButton.json ? elementId(findButton.json) : null;
  if (buttonId) {
    await driveStep("element.button.click", "POST", `/element/${buttonId}/click`, {});
    await driveStep("execute/sync.read-click-probe", "POST", "/execute/sync", {
      script: "return window.__wd_click === undefined ? 'NO_CLICK_EVENT' : window.__wd_click;",
      args: [],
    });
  } else {
    record("element.button.click", { skipped: "no element id returned by element.find.button" });
  }

  const findEl = await driveStep("element.find.body", "POST", "/element", {
    using: "css selector",
    value: "body",
  });
  const bodyId = findEl.json ? elementId(findEl.json) : null;
  if (bodyId) {
    await driveStep("element.body.text", "GET", `/element/${bodyId}/text`);
    await driveStep("element.body.click", "POST", `/element/${bodyId}/click`, {});
    await driveStep("element.body.value", "GET", `/element/${bodyId}/value`);
    await driveStep("element.body.attribute.class", "GET", `/element/${bodyId}/attribute/class`);
    await driveStep("element.body.displayed", "GET", `/element/${bodyId}/displayed`);
    await driveStep("element.body.enabled", "GET", `/element/${bodyId}/enabled`);
  } else {
    record("element.body.text", { skipped: "no element id returned by element.find.body" });
  }

  // `findElements` (plural) — WDIO's `$$` and Vitest's query helpers use it.
  await driveStep("elements.find.divs", "POST", "/elements", {
    using: "css selector",
    value: "div",
  });

  const findInput = await driveStep("element.find.input", "POST", "/element", {
    using: "css selector",
    value: "#wd-probe-input",
  });
  const inputId = findInput.json ? elementId(findInput.json) : null;
  if (inputId) {
    await driveStep("element.input.value", "GET", `/element/${inputId}/value`);
    await driveStep("element.input.text", "GET", `/element/${inputId}/text`);
    // The W3C-era replacement for the legacy `/value` endpoint.
    await driveStep("element.input.property.value", "GET", `/element/${inputId}/property/value`);
    // Keyboard input: does it land in the document, and via which CDP method?
    await driveStep("element.input.clear", "POST", `/element/${inputId}/clear`, {});
    await driveStep("element.input.send-keys", "POST", `/element/${inputId}/value`, {
      text: "wd-typed",
    });
    await driveStep("execute/sync.read-input-value", "POST", "/execute/sync", {
      script: "return document.getElementById('wd-probe-input').value;",
      args: [],
    });
  } else {
    record("element.input.value", { skipped: "no element id returned by element.find.input" });
  }

  const screenshot = await driveStep("screenshot", "GET", "/screenshot");
  if (screenshot.ok && screenshot.json?.value) {
    const bytes = Buffer.from(screenshot.json.value, "base64");
    const file = join(DRIVER_OUT, "screenshot.png");
    writeFileSync(file, bytes);
    const isPng = bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
    let width = null;
    let height = null;
    if (isPng && bytes.length >= 24) {
      width = bytes.readUInt32BE(16);
      height = bytes.readUInt32BE(20);
    }
    results.screenshot = { file, bytes: bytes.length, isPng, width, height };
    log(`       screenshot: ${bytes.length} bytes, png=${isPng}, ${width}x${height} -> ${file}`);
  }

  await driveStep("cookie.get.initial", "GET", "/cookie");
  const setCookie = await driveStep("cookie.set", "POST", "/cookie", {
    cookie: { name: "wd_probe", value: "1", domain: "tauri.localhost", path: "/" },
  });
  if (!setCookie.ok) {
    await driveStep("cookie.set.nodomain", "POST", "/cookie", {
      cookie: { name: "wd_probe", value: "1" },
    });
  }
  await driveStep("cookie.get.after", "GET", "/cookie");

  await driveStep("window.rect.get", "GET", "/window/rect");
  await driveStep("window.rect.set", "POST", "/window/rect", { width: 900, height: 700 });

  await driveStep("timeouts.get", "GET", "/timeouts");
  await driveStep("timeouts.set", "POST", "/timeouts", { script: 30000, pageLoad: 30000, implicit: 0 });

  await driveStep("window.handles", "GET", "/window/handles");
  await driveStep("close-session", "DELETE", "");
}

// ------------------------------------------- 5. which CDP method Qt rejected
function analyseRejectedCdp() {
  let text = "";
  try {
    text = readFileSync(logPath, "utf8");
  } catch {
    return;
  }
  const lines = text.split("\n");

  // (a) CDP commands ChromeDriver sent, with how often.
  const census = new Map();
  for (const line of lines) {
    const found = line.match(/DevTools WebSocket Command: ([\w.]+)/);
    if (found) census.set(found[1], (census.get(found[1]) || 0) + 1);
  }
  results.cdpCommands = Object.fromEntries([...census.entries()].sort((a, b) => b[1] - a[1]));

  // (b) CDP-level rejections: an error response to a specific method.
  const cdpFailures = new Map();
  for (const line of lines) {
    const found = line.match(
      /DevTools WebSocket Response: ([\w.]+) .*\{.*"code":(-?\d+),"message":"([^"]*)"/,
    );
    if (found) {
      cdpFailures.set(found[1] + " " + found[2], {
        method: found[1],
        code: Number(found[2]),
        message: found[3],
      });
    }
  }
  results.cdpFailures = [...cdpFailures.values()];

  // (c) refusals ChromeDriver made on its own (no CDP command involved).
  const refusals = new Map();
  for (const line of lines) {
    const found = line.match(/RESPONSE (\w+) ERROR (.*)/);
    if (found) refusals.set(found[1] + " " + found[2], { command: found[1], error: found[2] });
  }
  results.driverRefusals = [...refusals.values()];

  if (results.cdpFailures.length) {
    log("\n==> CDP methods QtWebEngine rejected (from chromedriver --verbose log):");
    for (const failure of results.cdpFailures) {
      log(`    ${failure.method}: code=${failure.code} message="${failure.message}"`);
    }
  }
  if (results.driverRefusals.length) {
    log("==> refusals made by ChromeDriver itself (no CDP command sent):");
    for (const refusal of results.driverRefusals) {
      log(`    ${refusal.command}: ${refusal.error}`);
    }
  }
  if (Object.keys(results.cdpCommands).length) {
    log("==> CDP commands ChromeDriver sent: " + JSON.stringify(results.cdpCommands));
  }
}

// ----------------------------------------------------------------- main flow
let driver;
let fixup = null;
try {
  const cdpOk = await probeCdp();
  if (!cdpOk) {
    results.verdict = "cannot attach: no CDP endpoint";
    throw new Error(`no DevTools endpoint at ${CDP_BASE}`);
  }

  if (FIXUP_VERSION) {
    fixup = await startVersionFixup({
      target: CDP,
      port: FIXUP_PORT,
      browserVersion: FIXUP_VERSION,
    });
    DEBUGGER_ADDRESS = `127.0.0.1:${fixup.port}`;
    const viaProxy = await http("GET", `http://${DEBUGGER_ADDRESS}/json/version`);
    results.fixup = {
      enabled: true,
      version: FIXUP_VERSION,
      address: DEBUGGER_ADDRESS,
      target: CDP,
      jsonVersion: viaProxy.json ?? viaProxy.text,
    };
    log(`\nversion-fixup proxy 127.0.0.1:${fixup.port} -> ${CDP}, Browser rewritten to ${FIXUP_VERSION}`);
    log(`GET http://${DEBUGGER_ADDRESS}/json/version (through proxy) -> ${viaProxy.status}`);
    log(viaProxy.text.trim());
  } else {
    results.fixup = { enabled: false, address: CDP };
  }

  driver = startChromedriver();
  const status = await waitForDriver();
  results.chromedriver.status = status.json ?? status.text;
  const browserString = results.cdp.version?.body?.Browser || "";
  const browserMatch = browserString.match(/Chrome\/(\d+)/);
  results.chromedriver.browser = browserString;
  results.chromedriver.browserMajor = browserMatch ? Number(browserMatch[1]) : null;
  results.chromedriver.versionMismatch =
    results.chromedriver.major !== null &&
    results.chromedriver.browserMajor !== null &&
    results.chromedriver.major !== results.chromedriver.browserMajor;
  log(`\nchromedriver ${results.chromedriver.path} (expects Chrome ${results.chromedriver.major})`);
  log(`target reports ${browserString} (Chromium ${results.chromedriver.browserMajor}) — mismatch=${results.chromedriver.versionMismatch}`);
  log(`GET ${DRIVER_BASE}/status -> ${status.status}`);
  log(status.text.trim());

  results.session.request = {
    capabilities: {
      alwaysMatch: {
        browserName: "chrome",
        "goog:chromeOptions": { debuggerAddress: DEBUGGER_ADDRESS },
      },
    },
  };
  const created = await http("POST", `${DRIVER_BASE}/session`, results.session.request);
  results.session.response = created.json ?? created.text;
  results.session.httpStatus = created.status;
  log(`\nPOST ${DRIVER_BASE}/session -> ${created.status}`);
  log(created.text.trim());

  const sessionId = created.json?.value?.sessionId;
  if (sessionId) {
    results.session.id = sessionId;
    results.session.capabilities = created.json.value.capabilities;
    log(`\nsession created: ${sessionId} — driving W3C command battery\n`);
    await driveCommands();
    results.verdict = "attach works";
  } else {
    results.verdict = "cannot attach: session creation failed";
  }
} catch (err) {
  results.error = String(err?.stack || err);
  log(`\nprobe error: ${err}`);
} finally {
  analyseRejectedCdp();
  if (fixup) {
    if (results.fixup) results.fixup.observedRequests = fixup.requests;
    await fixup.close();
  }
  if (driver) {
    if (results.session.id) {
      // Best effort; already attempted above when the battery ran.
      await http("DELETE", `${DRIVER_BASE}/session/${results.session.id}`).catch(() => {});
    }
    if (!KEEP_DRIVER) {
      driver.child.kill("SIGTERM");
      await sleep(300);
      if (driver.child.exitCode === null) driver.child.kill("SIGKILL");
    }
  }
  writeFileSync(join(DRIVER_OUT, "results.json"), JSON.stringify(results, (key, value) =>
    typeof value === "function" ? undefined : value, 2) + "\n");
  const passed = results.steps.filter((step) => step.ok === true).length;
  const failed = results.steps.filter((step) => step.ok === false).length;
  log(`\n==> ${passed} step(s) ok, ${failed} step(s) failed; results -> ${join(DRIVER_OUT, "results.json")}`);
  process.exit(results.cdp.version ? 0 : 2);
}
