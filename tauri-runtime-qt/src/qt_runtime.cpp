// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

// Runtime-level Qt glue: native platform handles and the event-loop pump that
// drains Rust's user-event queue into the sink installed by `run()`
// (see src/ops/runtime.rs for the Rust half and src/qt_runtime.hpp for the
// threading contract).

#include "qt_runtime.hpp"

#include "qt_window.hpp"  // qt_window_winid: the widget-registry lookup

// cxx-generated header of the bridge in `src/bridge/runtime.rs`; declares
// `qt_on_pump_tick` and pulls in `qt_runtime.hpp`.
#include "tauri-runtime-qt/src/bridge/runtime.rs.h"

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QMetaObject>
#include <QtCore/QPointer>
#include <QtCore/QSemaphore>
#include <QtCore/QString>
#include <QtCore/QThread>
#include <QtCore/QTimer>
#include <QtGui/QGuiApplication>
#include <QtGui/qguiapplication_platform.h>
#include <QtWidgets/QApplication>
#include <QtWidgets/QWidget>

#include <csignal>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <memory>

namespace {

/// How long an off-GUI-thread read waits for the GUI thread before reporting
/// failure (callers turn 0 into `HandleError::Unavailable`). The GUI thread
/// runs the event loop, so it answers in microseconds unless the application is
/// wedged.
constexpr int kGuiThreadTimeoutMs = 5000;

/// Pump cadence. Wake-ups posted by `qt_runtime_wake()` drain immediately; the
/// timer is the safety net for events whose wake-up predates the
/// `QCoreApplication` (they stay queued until the first tick).
constexpr int kPumpIntervalMs = 16;

QPointer<QTimer> g_pumpTimer;

QCoreApplication* application() {
    return QCoreApplication::instance();
}

#ifdef TAURIQT_COVERAGE
// Provided by the LLVM profile runtime rustc links for `-Cinstrument-coverage`
// — the same runtime instance the clang-instrumented C++ translation units
// resolve against (see build.rs), so one call flushes Rust and C++ alike.
// Declared by hand: compiler-rt's InstrProfiling.h is not on the include path
// of an ordinary Qt build.
extern "C" int __llvm_profile_write_file(void);

/// Flushes every instrumented translation unit and then dies of the signal with
/// its default disposition, so the exit status still says "killed".
///
/// Coverage runs are harness-driven: the harness stops the app (or the session
/// it lives in) the moment it sees its verdict, and a signal taking the default
/// action would skip the atexit hook the profile runtime installs, losing
/// everything the run accumulated. Every signal that can end such a run is
/// therefore armed with this flush: `SIGTERM`/`SIGINT` for an explicit stop,
/// `SIGHUP`/`SIGQUIT`/`SIGPIPE` for a dying compositor or display connection,
/// `SIGABRT` for a `qFatal()` in Qt. Coverage builds only.
void flushProfilesAndReraise(int signal_number) {
    std::fprintf(stderr, "[tauri-runtime-qt] coverage: flushing the profile (signal %d)\n",
                 signal_number);
    (void)__llvm_profile_write_file();
    std::signal(signal_number, SIG_DFL);
    std::raise(signal_number);
}
#endif

/// `TAURIQT_EXIT_AFTER_MS=<n>`: leave the event loop through
/// `QCoreApplication::exit(0)` n milliseconds after the pump starts, so a
/// harness-driven run ends by itself and the profile runtime's atexit flush
/// gets to run (`timeout`-killing the app would discard the profile). In a
/// coverage build, arm the profile-flushing signal handlers as well. GUI thread
/// only; unset or unparsable value: no timer, no handlers.
void installExitHooks(QCoreApplication* app) {
    const QByteArray value = qgetenv("TAURIQT_EXIT_AFTER_MS");
    if (!value.isEmpty()) {
        bool parsed = false;
        const int after_ms = value.toInt(&parsed);
        if (parsed && after_ms > 0) {
            QTimer::singleShot(after_ms, app, []() {
                std::fprintf(stderr,
                             "[tauri-runtime-qt] exit-after-ms: leaving the event loop\n");
                QCoreApplication::exit(0);
            });
        }
    }
#ifdef TAURIQT_COVERAGE
    const int flush_signals[] = {SIGTERM, SIGINT, SIGHUP, SIGQUIT, SIGABRT, SIGPIPE};
    for (const int signal_number : flush_signals) {
        std::signal(signal_number, flushProfilesAndReraise);
    }
#endif
}

#ifdef TAURIQT_COVERAGE
/// Default period of the dump installed by `installPeriodicFlush`.
constexpr int kProfileFlushMs = 1000;

/// Dumps the profile of the running process every `TAURIQT_PROFILE_FLUSH_MS`
/// (default `kProfileFlushMs`, `0` disables it).
///
/// Not every run ends where it should: a harness can stop the app's session
/// under it, and a process killed with `SIGKILL` — or reached before one of the
/// handlers above — never runs the atexit hook the profile runtime installs, so
/// nothing would be written at all. The dump is cumulative and merges into
/// `LLVM_PROFILE_FILE`, so the last dump before the kill is what gets reported;
/// the interval therefore bounds how much of a killed run can be missing.
/// GUI thread only (called from `installPumpTimer`).
void installPeriodicFlush(QCoreApplication* app) {
    const QByteArray value = qgetenv("TAURIQT_PROFILE_FLUSH_MS");
    bool parsed = false;
    const int interval_ms = value.isEmpty() ? kProfileFlushMs : value.toInt(&parsed);
    if ((!value.isEmpty() && !parsed) || interval_ms <= 0) {
        return;
    }
    auto* timer = new QTimer(app);
    timer->setTimerType(Qt::CoarseTimer);
    timer->setInterval(interval_ms);
    QObject::connect(timer, &QTimer::timeout, app, []() { (void)__llvm_profile_write_file(); });
    timer->start();
}
#endif

/// Installs the drain timer; GUI thread only.
void installPumpTimer() {
    QCoreApplication* app = application();
    if (app == nullptr || !g_pumpTimer.isNull()) {
        return;
    }
    installExitHooks(app);
#ifdef TAURIQT_COVERAGE
    installPeriodicFlush(app);
#endif
    auto* timer = new QTimer(app);
    timer->setTimerType(Qt::CoarseTimer);
    timer->setInterval(kPumpIntervalMs);
    QObject::connect(timer, &QTimer::timeout, app, []() { qt_on_pump_tick(); });
    timer->start();
    g_pumpTimer = timer;
}

/// Runs `fn` on the GUI thread and waits for it; false when the GUI thread did
/// not answer in time. The semaphore is kept alive by the shared pointer even
/// when the wait times out, so a late call is still safe.
template <typename Fn>
bool runOnGuiThreadAndWait(QCoreApplication* app, Fn fn) {
    if (QThread::currentThread() == app->thread()) {
        fn();
        return true;
    }
    const auto done = std::make_shared<QSemaphore>();
    QMetaObject::invokeMethod(app, [done, fn]() mutable {
        fn();
        done->release();
    }, Qt::QueuedConnection);
    return done->tryAcquire(1, kGuiThreadTimeoutMs);
}

/// The platform connection Qt opened: `kind` matches `qt_runtime_platform_kind()`,
/// `pointer` is the `xcb_connection_t*` / `Display*` / `wl_display*`.
struct PlatformConnection {
    std::int32_t kind = 0;
    std::uint64_t pointer = 0;
};

/// Reads the platform connection Qt opened.
///
/// The plugin name decides whether the handles are real: only the `xcb`
/// (X11) and `wayland` plugins own a window system connection whose windows
/// can be described as raw handles. Other plugins answer the same native
/// interfaces with unrelated objects — the `offscreen` plugin, for instance,
/// implements `QX11Application` with the private `Display*` it opens for GLX
/// and returns counter values (1, 2, …) from `QWidget::winId()` — so they
/// report "unknown" instead of handing out values that do not describe a
/// window.
///
/// The platform integration and the connection it returns are created once by
/// the `QGuiApplication` constructor and never replaced, so this only reads
/// immutable state: `resolveInterface()` (which backs `nativeInterface<T>()`)
/// is a `dynamic_cast` against `platformNativeInterface()`. No widget or
/// event-loop state is touched, so the read is safe from any thread, including
/// before the event loop starts.
PlatformConnection platformConnection() {
    PlatformConnection connection;
    auto* app = qobject_cast<QGuiApplication*>(application());
    if (app == nullptr) {
        return connection;
    }

    const QString platformName = QGuiApplication::platformName();
    if (platformName == QStringLiteral("wayland")) {
#if QT_CONFIG(wayland)
        if (auto* wayland = app->nativeInterface<QNativeInterface::QWaylandApplication>()) {
            if (wayland->display() != nullptr) {
                connection.kind = 3;
                connection.pointer = reinterpret_cast<std::uint64_t>(wayland->display());
            }
        }
#endif
        return connection;
    }

    if (platformName == QStringLiteral("xcb")) {
#if QT_CONFIG(xcb)
        if (auto* x11 = app->nativeInterface<QNativeInterface::QX11Application>()) {
            if (x11->connection() != nullptr) {
                connection.kind = 1;
                connection.pointer = reinterpret_cast<std::uint64_t>(x11->connection());
            } else if (x11->display() != nullptr) {
                connection.kind = 2;
                connection.pointer = reinterpret_cast<std::uint64_t>(x11->display());
            }
        }
#endif
        return connection;
    }

    // Any other plugin (offscreen, minimal, vnc, eglfs, linuxfb, …): no
    // window-system handles.
    return connection;
}

}  // namespace

std::int32_t qt_runtime_platform_kind() {
    return platformConnection().kind;
}

rust::String qt_runtime_platform_name() {
    if (QGuiApplication::instance() == nullptr) {
        return rust::String();
    }
    return rust::String(QGuiApplication::platformName().toUtf8().constData());
}

std::uint64_t qt_runtime_display_pointer() {
    return platformConnection().pointer;
}

std::uint64_t qt_runtime_window_winid(std::uint64_t id) {
    QCoreApplication* app = application();
    if (app == nullptr) {
        return 0;
    }
    const auto winid = std::make_shared<std::uint64_t>(0);
    const bool answered = runOnGuiThreadAndWait(app, [id, winid]() {
        *winid = qt_window_winid(id);
    });
    return answered ? *winid : 0;
}

bool qt_runtime_has_application() {
    return application() != nullptr;
}

void qt_runtime_wake() {
    QCoreApplication* app = application();
    if (app == nullptr) {
        return;  // no event loop to wake; the first pump tick drains the queue
    }
    QMetaObject::invokeMethod(app, []() { qt_on_pump_tick(); }, Qt::QueuedConnection);
}

bool qt_runtime_start_pump() {
    QCoreApplication* app = application();
    if (app == nullptr) {
        return false;
    }
    if (QThread::currentThread() == app->thread()) {
        installPumpTimer();
    } else {
        QMetaObject::invokeMethod(app, []() { installPumpTimer(); }, Qt::QueuedConnection);
    }
    return true;
}

void qt_runtime_exit(std::int32_t code) {
    QCoreApplication* app = application();
    if (app == nullptr) {
        return;
    }
    if (QThread::currentThread() == app->thread()) {
        QCoreApplication::exit(code);
        return;
    }
    QMetaObject::invokeMethod(app, [code]() { QCoreApplication::exit(code); },
                              Qt::QueuedConnection);
}

void qt_runtime_process_events() {
    QCoreApplication* app = application();
    if (app == nullptr) {
        return;
    }
    if (QThread::currentThread() == app->thread()) {
        QCoreApplication::processEvents();
        return;
    }
    QMetaObject::invokeMethod(app, []() { QCoreApplication::processEvents(); },
                              Qt::QueuedConnection);
}

rust::String qt_runtime_debug_native_state() {
    const PlatformConnection connection = platformConnection();

    const QString platformName =
        qGuiApp != nullptr ? QGuiApplication::platformName() : QStringLiteral("<no application>");
    QString text = QStringLiteral("platform=%1 kind=%2 display=0x%3")
                       .arg(platformName)
                       .arg(connection.kind)
                       .arg(connection.pointer, 0, 16);

    if (qobject_cast<QApplication*>(application()) != nullptr) {
        const QWidgetList windows = QApplication::topLevelWidgets();
        text += QStringLiteral(" topLevelWidgets=%1").arg(windows.size());
        for (QWidget* window : windows) {
            text += QStringLiteral(" [winId=0x%1 visible=%2 title=\"%3\"]")
                        .arg(static_cast<quintptr>(window->winId()), 0, 16)
                        .arg(window->isVisible() ? 1 : 0)
                        .arg(window->windowTitle());
        }
    } else {
        text += QStringLiteral(" (no QApplication)");
    }

    const QByteArray utf8 = text.toUtf8();
    return rust::String(utf8.constData(), static_cast<std::size_t>(utf8.size()));
}
