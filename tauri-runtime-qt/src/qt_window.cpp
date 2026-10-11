// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Top-level window registry for the Qt runtime.
//!
//! Owns one `QWidget` per Tauri window, keyed by the stable handle handed to
//! Rust. All entry points run on the Qt GUI thread (the Rust side marshals
//! through `QMetaObject::invokeMethod`); each one tolerates an unknown handle.

#include "qt_window.hpp"

// cxx-generated header of the bridge in `src/qt.rs`; declares the
// `qt_on_window_event` callback with the linkage cxx expects, so it must not
// be hand-declared here.
#include "tauri-runtime-qt/src/qt.rs.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QPointer>
#include <QtCore/QString>
#include <QtGui/QCloseEvent>
#include <QtGui/QGuiApplication>
#include <QtGui/QMoveEvent>
#include <QtGui/QResizeEvent>
#include <QtGui/QScreen>
#include <QtGui/QWindow>
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

    // Changing window flags re-parents the widget (hiding it) and may replace
    // the platform QWindow: restore visibility and the signal connection.
    const bool was_visible = window->isVisible();
    window->setWindowFlag(Qt::FramelessWindowHint, !decorations);
    if (was_visible) {
        window->show();
    }
    window->attach();
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
