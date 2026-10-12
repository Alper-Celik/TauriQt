<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Integrations — running other people's apps on this runtime

Our own harnesses (`scripts/`, the flake checks) grade the app we wrote, which is why they
are suspect by construction. The point of this directory is the opposite: run **someone
else's** Tauri app on `tauri-runtime-qt` and let *their* code and *their* tests be the
judge. Readest is the first one; the layout is deliberately generic so the next app is a
directory plus an entry in `apps.json`.

## Layout of one app

```
integrations/<app>/
  README.md          what was measured, what works, what does not (and why)
  <app>.patch        the diff against one pinned commit of that app — a patch, not a fork
  apply.sh           clone that commit outside this repo, apply the patch, wire the runtime path
  run.sh             build + run the app on this runtime, with its DevTools port open
  check.mjs          dependency-free assertions over CDP (page rendered, invoke() reaches Rust)
  e2e.sh             optional: bridge for the app's own driver-based suite
  ci.sh              one entry point CI calls: apply, install, assert, tear down
```

`ci.sh` is what CI runs, so an app is "in CI" exactly when `apps.json` lists it.

## How CI runs them

`.github/workflows/ci.yml` job `apps-list` reads `integrations/apps.json` and feeds a matrix
to job `apps`, which runs each entry's `ci.sh` inside this repo's dev shell
(`nix develop -c …`) and folds the result into the same summary table as the flake checks.

The heavy apps do **not** run on every push: the `apps` job is gated to `schedule`
(nightly) and `workflow_dispatch`. A full Readest run is a 1000-crate Rust build plus a
Next.js install — worth having, not worth blocking a PR on. Their cheap stages (the app's
own unit tests) belong in the same job for the same reason.

## What every app integration must respect

- **Pin one commit.** `ref` in `apps.json` and in the app's `apply.sh` must agree; the patch
  is expected to rot and to be re-derived against a newer commit rather than forked.
- **Check the app out outside this repo.** Our root is a cargo workspace, so a checkout
  nested inside it makes cargo resolve that app's vendored crates against *our* workspace
  root (the failure looks like `workspace.package.edition was not defined`). `apply.sh`
  refuses a nested path.
- **Expect several GB and a long build.** The checkout carries `node_modules` and a Rust
  `target/`; a small filesystem fails mid-build with `No space left on device`. CI sets
  `READEST_CHECKOUT` into the runner's temp dir for this reason.
- **Prefer CDP to WebDriver.** QtWebEngine implements Chrome DevTools Protocol well enough
  for attaching to the page the app already owns, but it cannot create a *second* target
  (`Target.createTarget`), and ChromeDriver requires one — so WebDriver-driven suites
  (`tauri-plugin-webdriver`, WebdriverIO configs) are protocol-blocked here. See
  `docs/cdp-support-matrix.md`, `docs/webdriver-attach-findings.md`.
- **Say what did not work.** Every app README has a "measured results" table including the
  parts that fail; an integration that only reports successes is not evidence.

## Adding an app

1. `mkdir integrations/<app>` and write `apply.sh` / `run.sh` / `check.mjs` following the
   shape above (copy the Readest ones — they are the reference implementation).
2. Write `ci.sh` so that it (a) prepares the checkout, (b) runs at least one *app-authored*
   assertion (their test suite, or their app actually booting and answering `invoke()`),
   (c) always tears down what it started, and (d) exits non-zero on failure.
3. Add an entry to `apps.json` with the pinned `ref`, the systems you have actually run it
   on, and the stages it asserts.
4. Record the measured result in the app's `README.md`, failures included.
