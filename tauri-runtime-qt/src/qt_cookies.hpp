// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <rust/cxx.h>

// Cookie mirroring entry points (see src/bridge/cookies.rs). Handles are
// `QWebEngineView*` values (the webview native handle); the worker implements
// these in qt_cookies.cpp.
