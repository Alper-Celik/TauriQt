// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <rust/cxx.h>

/// Creates the `QApplication` and registers the Tauri URL schemes.
/// Returns 0 on success, non-zero if a QApplication already exists.
int qt_init(int argc);

/// Runs the Qt event loop and returns its exit code.
int qt_run();

/// Qt version string ("6.11.2").
rust::String qt_version();
