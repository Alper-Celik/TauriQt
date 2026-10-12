// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Top-level window registry for the Qt runtime.
//!
//! Owns one `QWidget` per Tauri window, keyed by the stable handle handed to
//! Rust. All entry points run on the Qt GUI thread (the runtime dispatchers
//! call them from the thread that owns the window) and each one tolerates an
//! unknown handle.

#include "qt_window.hpp"

// cxx-generated header of the bridge in `src/qt.rs`; declares the
// `qt_on_window_event` callback with the linkage cxx expects, so it must not
// be hand-declared here.
#include "tauri-runtime-qt/src/qt.rs.h"
#include "tauri-runtime-qt/src/bridge/window.rs.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QPointer>
#include <QtCore/QSize>
#include <QtCore/QString>
#include <QtCore/qnumeric.h>
#include <QtGui/QColor>
#include <QtGui/QCloseEvent>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtGui/QImage>
#include <QtGui/QMoveEvent>
#include <QtGui/QPalette>
#include <QtGui/QPixmap>
#include <QtGui/QResizeEvent>
#include <QtGui/QScreen>
#include <QtGui/QStyleHints>
#include <QtGui/QWindow>
#include <QtWidgets/QApplication>
#include <QtWidgets/QLayout>
#include <QtWidgets/QStyle>
#include <QtWidgets/QVBoxLayout>
#include <QtWidgets/QWidget>

#include <cstddef>
#include <cstdint>
#include <string>
#include <unordered_map>

namespace {

// Event kinds of `qt_on_window_event` (see qt_window.hpp / src/qt.rs).
constexpr std::int32_t kEventCloseRequested = 0;
constexpr std::int32_t kEventFocused = 1;
constexpr std::int32_t kEventUnfocused = 2;
constexpr std::int32_t kEventResized = 3;
constexpr std::int32_t kEventMoved = 4;

/// Top-level window widget. Reports the window events tauri-runtime's
/// `on_window_event` needs; it deliberately has no `Q_OBJECT` (build.rs does
/// not run moc, so no meta-object code may be required by this class).
class TauriWindow final : public QWidget {
public:
    TauriWindow(std::uint64_t id, const QString& label)
        : QWidget(nullptr, Qt::Window), m_id(id) {
        setObjectName(label);
    }

    /// Connects the QWindow-level notifications. Must run on the GUI thread.
    /// The platform QWindow is recreated when the window flags change (e.g.
    /// `qt_window_set_decorations`), so this re-connects whenever the native
    /// handle differs from the one we are attached to; safe to call often.
    void attach() {
        QWindow* handle = windowHandle();
        if (handle == nullptr) {
            winId();  // force the native window so windowHandle() is valid
            handle = windowHandle();
            if (handle == nullptr) {
                return;  // still not native: retry on the next attach()
            }
        }
        if (handle == m_attached_handle) {
            return;  // already connected to this platform window
        }
        m_attached_handle = handle;  // QPointer: nulls out if the handle dies
        m_ready = true;              // the handle is known to Rust from here on
        // `QWindow::activeChanged` carries no argument (Qt 6): read the state.
        QObject::connect(handle, &QWindow::activeChanged, this, [this]() {
            if (QWindow* current = m_attached_handle) {
                emit_event(current->isActive() ? kEventFocused : kEventUnfocused, std::string());
            }
        });
    }

    /// Enters or leaves fullscreen. Leaving restores the maximized state the
    /// window had before it went fullscreen (`showNormal()` would drop it).
    /// The request is mirrored because a window manager confirms
    /// `Qt::WindowFullScreen` asynchronously — `isFullScreen()` can still lag
    /// behind — while `qt_window_is_fullscreen` keeps reporting Qt's view.
    /// The state of a hidden window is applied when it is next shown.
    void set_fullscreen(bool fullscreen) {
        if (fullscreen) {
            if (m_fullscreen_requested && isFullScreen()) {
                return;
            }
            m_pre_fullscreen_state = windowState() & (Qt::WindowMinimized | Qt::WindowMaximized);
            m_fullscreen_requested = true;
            setWindowState(windowState() | Qt::WindowFullScreen);
        } else {
            if (!m_fullscreen_requested && !isFullScreen()) {
                return;
            }
            m_fullscreen_requested = false;
            setWindowState((windowState() & ~Qt::WindowFullScreen) | m_pre_fullscreen_state);
        }
        if (isVisible()) {
            show();  // apply the new state immediately
        }
    }

    /// Qt exposes neither a resizable getter nor a setter; the size hints are
    /// what a window manager honours, so disabling resizing pins the window to
    /// its current size (`minimumSize == maximumSize`) and remembers the
    /// constraints in force so that re-enabling restores them.
    void set_resizable(bool resizable) {
        if (resizable) {
            if (!m_pinned_by_resizable) {
                return;
            }
            setMinimumSize(m_pinned_min);
            setMaximumSize(m_pinned_max);
            m_pinned_by_resizable = false;
        } else {
            if (m_pinned_by_resizable) {
                return;
            }
            m_pinned_min = minimumSize();
            m_pinned_max = maximumSize();
            m_pinned_by_resizable = true;
            setMinimumSize(size());
            setMaximumSize(size());
        }
    }

    /// A window is resizable exactly while its size hints do not pin both
    /// dimensions to one value (`set_resizable(false)` installs
    /// `minimumSize == maximumSize`; explicit equal constraints pin it too).
    bool is_resizable() const {
        return minimumSize() != maximumSize();
    }

    /// Sets the minimum size from logical pixels. While resizing is disabled
    /// the remembered pre-pin constraint is kept coherent, so that
    /// `set_resizable(true)` restores what the caller last asked for.
    void set_min_size(double width, double height) {
        const QSize size(qRound(width), qRound(height));
        if (m_pinned_by_resizable) {
            m_pinned_min = size;
        }
        setMinimumSize(size);
    }

    /// Sets the maximum size from logical pixels.
    void set_max_size(double width, double height) {
        const QSize size(qRound(width), qRound(height));
        if (m_pinned_by_resizable) {
            m_pinned_max = size;
        }
        setMaximumSize(size);
    }

protected:
    void closeEvent(QCloseEvent* event) override {
        // The Rust side observes the request and may re-issue `close` after
        // cleaning up; the callback is void, so the default handler runs.
        emit_event(kEventCloseRequested, std::string());
        QWidget::closeEvent(event);
    }

    void resizeEvent(QResizeEvent* event) override {
        QWidget::resizeEvent(event);
        emit_event(kEventResized, std::to_string(event->size().width()) + "x" +
                                     std::to_string(event->size().height()));
    }

    void moveEvent(QMoveEvent* event) override {
        QWidget::moveEvent(event);
        emit_event(kEventMoved, std::to_string(event->pos().x()) + "," +
                                    std::to_string(event->pos().y()));
    }

private:
    void emit_event(std::int32_t kind, const std::string& payload) {
        if (!m_ready) {
            return;  // handle not returned to Rust yet
        }
        qt_on_window_event(m_id, kind, rust::Str(payload));
    }

    std::uint64_t m_id;
    QPointer<QWindow> m_attached_handle;
    bool m_ready = false;
    /// Window state to restore when leaving fullscreen.
    Qt::WindowStates m_pre_fullscreen_state = Qt::WindowNoState;
    /// The last fullscreen state requested (the window manager confirms the
    /// change asynchronously, so `windowState()` alone is not enough).
    bool m_fullscreen_requested = false;
    /// Size constraints in force before `set_resizable(false)` pinned them.
    QSize m_pinned_min{0, 0};
    QSize m_pinned_max{QWIDGETSIZE_MAX, QWIDGETSIZE_MAX};
    bool m_pinned_by_resizable = false;
};

std::unordered_map<std::uint64_t, TauriWindow*>& registry() {
    static std::unordered_map<std::uint64_t, TauriWindow*> windows;
    return windows;
}

std::uint64_t next_handle = 0;

/// Lookup that never throws and never returns a dangling widget.
TauriWindow* find_window(std::uint64_t id) {
    if (id == 0) {
        return nullptr;
    }
    const auto it = registry().find(id);
    return it == registry().end() ? nullptr : it->second;
}

/// Applies new window flags, restoring visibility: changing a window's flags
/// re-parents the widget (hiding it) and may replace the platform `QWindow`,
/// so the event connections are re-established as well.
void apply_window_flags(TauriWindow* window, Qt::WindowFlags flags) {
    if (flags == window->windowFlags()) {
        return;  // a no-op flag change would needlessly hide and re-show it
    }
    const bool was_visible = window->isVisible();
    window->setWindowFlags(flags);
    if (was_visible) {
        window->show();
    }
    window->attach();
}

/// tauri's `ResizeDirection` numbering (as mapped by `lib.rs`) to Qt edges.
Qt::Edges edges_for_direction(std::int32_t direction) {
    switch (direction) {
        case 1:  // East
            return Qt::RightEdge;
        case 2:  // North
            return Qt::TopEdge;
        case 3:  // NorthEast
            return Qt::TopEdge | Qt::RightEdge;
        case 4:  // NorthWest
            return Qt::TopEdge | Qt::LeftEdge;
        case 5:  // South
            return Qt::BottomEdge;
        case 6:  // SouthEast
            return Qt::BottomEdge | Qt::RightEdge;
        case 7:  // SouthWest
            return Qt::BottomEdge | Qt::LeftEdge;
        case 8:  // West
            return Qt::LeftEdge;
        default:
            return Qt::Edges();  // unknown: no direction
    }
}

/// UTF-8 view of a QString for the Rust side.
rust::String to_rust_string(const QString& text) {
    const QByteArray utf8 = text.toUtf8();
    return rust::String(utf8.constData(), static_cast<std::size_t>(utf8.size()));
}

}  // namespace

uint64_t qt_window_create(rust::Str label, rust::Str title, int width, int height, bool visible,
                          bool decorations) {
    if (QCoreApplication::instance() == nullptr) {
        return 0;  // a QWidget must not be constructed before the QApplication
    }

    const std::uint64_t id = ++next_handle;
    auto* window = new TauriWindow(id, QString::fromUtf8(label.data(), static_cast<int>(label.size())));
    window->setWindowTitle(QString::fromUtf8(title.data(), static_cast<int>(title.size())));
    if (width > 0 && height > 0) {
        window->resize(width, height);
    }
    if (!decorations) {
        window->setWindowFlag(Qt::FramelessWindowHint, true);
    }

    registry().emplace(id, window);

    if (visible) {
        window->show();
    }
    window->attach();
    return id;
}

void qt_window_show(uint64_t id) {
    if (TauriWindow* window = find_window(id)) {
        window->show();  // focus/raise are separate tauri calls (set_focus)
        window->attach();
    }
}

void qt_window_hide(uint64_t id) {
    if (TauriWindow* window = find_window(id)) {
        window->hide();
    }
}

void qt_window_close(uint64_t id) {
    if (TauriWindow* window = find_window(id)) {
        window->close();  // delivers closeEvent -> kind 0, then hides
    }
}

void qt_window_set_title(uint64_t id, rust::Str title) {
    if (TauriWindow* window = find_window(id)) {
        window->setWindowTitle(QString::fromUtf8(title.data(), static_cast<int>(title.size())));
    }
}

void qt_window_set_size(uint64_t id, int width, int height) {
    if (TauriWindow* window = find_window(id); window != nullptr && width > 0 && height > 0) {
        window->resize(width, height);
    }
}

void qt_window_center(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }

    QScreen* screen = nullptr;
    if (QWindow* handle = window->windowHandle()) {
        screen = handle->screen();
    }
    if (screen == nullptr) {
        screen = QGuiApplication::screenAt(window->frameGeometry().center());
    }
    if (screen == nullptr) {
        screen = QGuiApplication::primaryScreen();
    }
    if (screen == nullptr) {
        return;
    }

    const QRect target = QStyle::alignedRect(Qt::LeftToRight, Qt::AlignCenter, window->size(),
                                             screen->availableGeometry());
    window->move(target.topLeft());
}

void qt_window_set_decorations(uint64_t id, bool decorations) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }

    Qt::WindowFlags flags = window->windowFlags();
    flags.setFlag(Qt::FramelessWindowHint, !decorations);
    apply_window_flags(window, flags);
}

void qt_window_set_maximized(uint64_t id, bool maximized) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }

    const Qt::WindowStates state = window->windowState();
    window->setWindowState(maximized ? (state | Qt::WindowMaximized) : (state & ~Qt::WindowMaximized));
}

bool qt_window_is_visible(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->isVisible();
}

double qt_window_scale_factor(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return 1.0;
    }
    if (QWindow* handle = window->windowHandle()) {
        return handle->devicePixelRatio();  // correct under Wayland fractional scaling
    }
    return window->devicePixelRatioF();
}

uint64_t qt_window_winid(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return 0;
    }
    return static_cast<std::uint64_t>(window->winId());
}

void qt_window_attach_webview(uint64_t window_id, uint64_t webview_native_id) {
    TauriWindow* window = find_window(window_id);
    if (window == nullptr || webview_native_id == 0) {
        return;
    }

    // The webview agent hands us its QWebEngineView as an opaque pointer; the
    // widget fills the window's client area, so it is added to a zero-margin
    // layout that keeps it in sync with every resize.
    auto* view = reinterpret_cast<QWidget*>(static_cast<quintptr>(webview_native_id));
    if (view == nullptr) {
        return;
    }

    view->setParent(window);
    QLayout* layout = window->layout();
    if (layout == nullptr) {
        auto* box = new QVBoxLayout(window);
        box->setContentsMargins(0, 0, 0, 0);
        box->setSpacing(0);
        layout = box;
    }
    layout->addWidget(view);
    view->show();
}

// ---- window worker: registry and platform ----

bool qt_window_alive(uint64_t id) {
    return find_window(id) != nullptr;
}

rust::String qt_window_platform() {
    return to_rust_string(QGuiApplication::platformName());
}

// ---- geometry (Qt device-independent pixels) ----

QtWindowSize qt_window_inner_size(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return {0.0, 0.0};
    }
    const QSize size = window->size();  // client area, excluding frame
    return {static_cast<double>(size.width()), static_cast<double>(size.height())};
}

QtWindowSize qt_window_outer_size(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return {0.0, 0.0};
    }
    const QSize size = window->frameGeometry().size();
    return {static_cast<double>(size.width()), static_cast<double>(size.height())};
}

QtWindowPoint qt_window_inner_position(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return {0.0, 0.0};
    }
    const QPoint origin = window->mapToGlobal(QPoint(0, 0));
    return {static_cast<double>(origin.x()), static_cast<double>(origin.y())};
}

QtWindowPoint qt_window_outer_position(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return {0.0, 0.0};
    }
    const QPoint origin = window->frameGeometry().topLeft();
    return {static_cast<double>(origin.x()), static_cast<double>(origin.y())};
}

// ---- state ----

rust::String qt_window_title(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return rust::String();
    }
    return to_rust_string(window->windowTitle());
}

int32_t qt_window_theme(uint64_t id) {
    if (find_window(id) == nullptr) {
        return 0;
    }
    // 6.5+: the platform's colour scheme; `Unknown` means it does not report one.
    switch (QGuiApplication::styleHints()->colorScheme()) {
        case Qt::ColorScheme::Light:
            return 1;
        case Qt::ColorScheme::Dark:
            return 2;
        case Qt::ColorScheme::Unknown:
        default:
            return 0;
    }
}

bool qt_window_is_fullscreen(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->isFullScreen();
}

bool qt_window_is_minimized(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->isMinimized();
}

bool qt_window_is_maximized(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->isMaximized();
}

bool qt_window_is_focused(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->isActiveWindow();
}

bool qt_window_is_decorated(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && !window->windowFlags().testFlag(Qt::FramelessWindowHint);
}

bool qt_window_is_resizable(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->is_resizable();
}

bool qt_window_is_enabled(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->isEnabled();
}

bool qt_window_is_always_on_top(uint64_t id) {
    TauriWindow* window = find_window(id);
    // The hint Qt hands the window manager. Wayland compositors never honour
    // it, and `qt_window_set_always_on_top` refuses to set it there, so the
    // flag reports the truth on both platforms.
    return window != nullptr && window->windowFlags().testFlag(Qt::WindowStaysOnTopHint);
}

bool qt_window_is_maximizable(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->windowFlags().testFlag(Qt::WindowMaximizeButtonHint);
}

bool qt_window_is_minimizable(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->windowFlags().testFlag(Qt::WindowMinimizeButtonHint);
}

bool qt_window_is_closable(uint64_t id) {
    TauriWindow* window = find_window(id);
    return window != nullptr && window->windowFlags().testFlag(Qt::WindowCloseButtonHint);
}

// ---- setters ----

void qt_window_set_fullscreen(uint64_t id, bool fullscreen) {
    if (TauriWindow* window = find_window(id)) {
        window->set_fullscreen(fullscreen);
    }
}

void qt_window_minimize(uint64_t id) {
    if (TauriWindow* window = find_window(id)) {
        window->showMinimized();
    }
}

void qt_window_unminimize(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }
    window->setWindowState(window->windowState() & ~Qt::WindowMinimized);
    // Best effort: a Wayland compositor decides whether activation happens.
    window->activateWindow();
}

void qt_window_set_min_size(uint64_t id, double width, double height) {
    if (TauriWindow* window = find_window(id)) {
        window->set_min_size(width, height);
    }
}

void qt_window_set_max_size(uint64_t id, double width, double height) {
    if (TauriWindow* window = find_window(id)) {
        window->set_max_size(width, height);
    }
}

bool qt_window_set_icon(uint64_t id, rust::Slice<const uint8_t> rgba, uint32_t width,
                        uint32_t height) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return false;
    }
    const std::size_t expected =
        static_cast<std::size_t>(width) * static_cast<std::size_t>(height) * 4u;
    if (width == 0 || height == 0 || rgba.size() != expected) {
        return false;
    }
    // The QImage is a view onto the Rust buffer, so `QPixmap::fromImage` must
    // happen inside this call (it converts/copies into an owned pixmap).
    const QImage image(rgba.data(), static_cast<int>(width), static_cast<int>(height),
                       QImage::Format_RGBA8888);
    window->setWindowIcon(QIcon(QPixmap::fromImage(image)));
    return true;
}

void qt_window_set_focus(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }
    // Both calls are activation requests: a Wayland compositor may refuse one
    // that did not follow user input, and Qt has no way to report that back.
    window->activateWindow();
    window->raise();
}

void qt_window_set_resizable(uint64_t id, bool resizable) {
    if (TauriWindow* window = find_window(id)) {
        window->set_resizable(resizable);
    }
}

void qt_window_set_enabled(uint64_t id, bool enabled) {
    if (TauriWindow* window = find_window(id)) {
        window->setEnabled(enabled);
    }
}

void qt_window_set_always_on_top(uint64_t id, bool on_top) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }
    Qt::WindowFlags flags = window->windowFlags();
    flags.setFlag(Qt::WindowStaysOnTopHint, on_top);
    if (on_top) {
        // The two stacking hints contradict each other; keep them exclusive.
        flags.setFlag(Qt::WindowStaysOnBottomHint, false);
    }
    apply_window_flags(window, flags);
}

void qt_window_set_always_on_bottom(uint64_t id, bool on_bottom) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }
    Qt::WindowFlags flags = window->windowFlags();
    flags.setFlag(Qt::WindowStaysOnBottomHint, on_bottom);
    if (on_bottom) {
        flags.setFlag(Qt::WindowStaysOnTopHint, false);
    }
    apply_window_flags(window, flags);
}

void qt_window_set_background_color(uint64_t id, uint8_t r, uint8_t g, uint8_t b, uint8_t a) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }
    QPalette palette = window->palette();
    palette.setColor(QPalette::Window, QColor(r, g, b, a));
    window->setPalette(palette);
    window->setAutoFillBackground(true);
}

void qt_window_reset_background_color(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return;
    }
    window->setPalette(QApplication::palette());
    window->setAutoFillBackground(false);
}

void qt_window_set_position(uint64_t id, int x, int y) {
    if (TauriWindow* window = find_window(id)) {
        // For a top-level widget `move` is the frame position on the desktop.
        window->move(x, y);
    }
}

bool qt_window_start_dragging(uint64_t id) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return false;
    }
    QWindow* handle = window->windowHandle();
    if (handle == nullptr) {
        return false;
    }
    return handle->startSystemMove();
}

bool qt_window_start_resize_dragging(uint64_t id, int32_t direction) {
    TauriWindow* window = find_window(id);
    if (window == nullptr) {
        return false;
    }
    QWindow* handle = window->windowHandle();
    if (handle == nullptr) {
        return false;
    }
    const Qt::Edges edges = edges_for_direction(direction);
    if (edges == Qt::Edges()) {
        return false;  // unknown direction: nothing to drag from
    }
    return handle->startSystemResize(edges);
}
