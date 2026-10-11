// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Screen/monitor/cursor provider for the Qt runtime.
//!
//! Reads Qt's own screen model (`QGuiApplication`, `QScreen`) and the cursor
//! position, so the other modules never touch them. Qt reports geometry in
//! device-independent (logical) global pixels; converting to physical pixels
//! with `scale_factor` is the Rust side's job.
//!
//! Every entry point may be called from any thread: work that has to run on the
//! GUI thread is marshalled there (see `on_gui_thread`).

#include "qt_screen.hpp"

// cxx-generated header of this bridge: declares the entry points and defines
// the shared structs they return.
#include "tauri-runtime-qt/src/bridge/screen.rs.h"
// `qt_window_winid()` (registry handle -> native window id), used by
// `qt_screen_for_window()` to find the window behind a handle. Included instead
// of hand-declaring it, so the linkage cxx expects is the one cxx generates.
#include "tauri-runtime-qt/src/bridge/window.rs.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QList>
#include <QtCore/QMetaObject>
#include <QtCore/QPoint>
#include <QtCore/QString>
#include <QtCore/QThread>
#include <QtCore/QtGlobal>
#include <QtGui/QCursor>
#include <QtGui/QGuiApplication>
#include <QtGui/QScreen>
#include <QtGui/QWindow>
#include <QtWidgets/QApplication>
#include <QtWidgets/QWidget>

#include <cstdint>
#include <string>
#include <type_traits>
#include <utility>

namespace {

/// The `QGuiApplication` of this process, or `nullptr` when there is none (or
/// only a plain `QCoreApplication`, which has no screens).
QGuiApplication* gui_application() {
    return qobject_cast<QGuiApplication*>(QCoreApplication::instance());
}

/// Runs `body` on the Qt GUI thread and returns its value.
///
/// On the GUI thread (the common case) `body` runs inline. From another thread
/// the call is queued to the application object — which lives on the GUI thread
/// — as a blocking queued invocation, so the caller gets the real value and the
/// Qt platform classes are only ever touched from their own thread.
///
/// A blocking invocation waits for the GUI thread to process events; it must
/// therefore not be used from inside a GUI-thread callback that blocks the
/// event loop on the calling thread.
template <typename F>
auto on_gui_thread(F&& body) -> std::invoke_result_t<F> {
    QCoreApplication* app = QCoreApplication::instance();
    if (app == nullptr || QThread::currentThread() == app->thread()) {
        return body();
    }

    using Result = std::invoke_result_t<F>;
    if constexpr (std::is_void_v<Result>) {
        QMetaObject::invokeMethod(app, std::forward<F>(body), Qt::BlockingQueuedConnection);
    } else {
        Result result{};
        QMetaObject::invokeMethod(
            app, [&body, &result]() { result = body(); }, Qt::BlockingQueuedConnection);
        return result;
    }
}

/// Copies one `QScreen` into the bridge representation (logical pixels).
QtScreenInfo screen_info(const QScreen* screen) {
    const QRect geometry = screen->geometry();
    const QRect available = screen->availableGeometry();

    QtScreenInfo info;
    info.valid = true;
    info.name = rust::String(screen->name().toStdString());
    info.x = geometry.x();
    info.y = geometry.y();
    info.width = geometry.width();
    info.height = geometry.height();
    info.available_x = available.x();
    info.available_y = available.y();
    info.available_width = available.width();
    info.available_height = available.height();
    info.scale_factor = screen->devicePixelRatio();
    return info;
}

}  // namespace

bool qt_screen_application_ready() {
    return on_gui_thread([] { return gui_application() != nullptr; });
}

rust::Vec<QtScreenInfo> qt_screen_monitors() {
    return on_gui_thread([]() -> rust::Vec<QtScreenInfo> {
        rust::Vec<QtScreenInfo> monitors;
        if (gui_application() == nullptr) {
            return monitors;  // Rust turns this into an error (see ready check)
        }
        for (QScreen* screen : QGuiApplication::screens()) {
            if (screen != nullptr) {
                monitors.push_back(screen_info(screen));
            }
        }
        return monitors;
    });
}

QtScreenInfo qt_screen_primary() {
    return on_gui_thread([]() -> QtScreenInfo {
        if (gui_application() == nullptr) {
            return QtScreenInfo{};
        }
        const QScreen* primary = QGuiApplication::primaryScreen();
        return primary != nullptr ? screen_info(primary) : QtScreenInfo{};
    });
}

QtScreenInfo qt_screen_at_point(double x, double y) {
    return on_gui_thread([x, y]() -> QtScreenInfo {
        if (gui_application() == nullptr) {
            return QtScreenInfo{};
        }
        const QPoint point(qRound(x), qRound(y));
        const QScreen* screen = QGuiApplication::screenAt(point);
        return screen != nullptr ? screen_info(screen) : QtScreenInfo{};
    });
}

QtWindowScreen qt_screen_for_window(std::uint64_t native) {
    return on_gui_thread([native]() -> QtWindowScreen {
        QtWindowScreen result;

        // 0 is the no-such-handle sentinel of `qt_window_winid`; the handle is
        // also what identifies the widget below.
        const std::uint64_t wid = qt_window_winid(native);
        if (wid == 0) {
            return result;  // window_known stays false
        }
        result.window_known = true;

        QWidget* window = nullptr;
        if (qobject_cast<QApplication*>(QCoreApplication::instance()) != nullptr) {
            // Only widgets that are already native can be the window behind
            // `wid`; widgets without a QWindow are skipped rather than forced.
            for (QWidget* candidate : QApplication::topLevelWidgets()) {
                QWindow* handle = candidate != nullptr ? candidate->windowHandle() : nullptr;
                if (handle != nullptr && static_cast<std::uint64_t>(handle->winId()) == wid) {
                    window = candidate;
                    break;
                }
            }
        }
        if (window == nullptr) {
            return result;  // the window has no native screen yet
        }

        // The screen of the window's platform handle, exactly as Qt exposes it
        // (`window->screen()` would fall back to the primary screen, which is a
        // guess rather than an answer).
        QWindow* window_handle = window->windowHandle();
        const QScreen* screen = window_handle != nullptr ? window_handle->screen() : nullptr;
        if (screen != nullptr) {
            result.screen = screen_info(screen);
        }
        return result;
    });
}

QtCursorInfo qt_screen_cursor_position() {
    return on_gui_thread([]() -> QtCursorInfo {
        QtCursorInfo info;

        if (gui_application() == nullptr) {
            return info;  // available stays false
        }
        if (QGuiApplication::platformName() == QStringLiteral("wayland")) {
            // Wayland has no global pointer position: clients only learn where
            // the cursor is from events on their own surfaces, so QCursor::pos()
            // cannot answer truthfully here.
            return info;
        }

        const QPoint position = QCursor::pos();
        const QScreen* screen = QGuiApplication::screenAt(position);
        info.available = true;
        info.x = position.x();
        info.y = position.y();
        info.scale_factor = screen != nullptr ? screen->devicePixelRatio() : 1.0;
        return info;
    });
}
