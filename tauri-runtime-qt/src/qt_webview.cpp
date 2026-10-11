// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

// Qt-backed webview registry (see qt_webview.hpp for the threading contract:
// every entry point here runs on the Qt GUI thread).
//
// The page-event callback `qt_on_webview_event` comes from the cxx bridge.

#include "qt_webview.hpp"

#include "qt_scheme.hpp"
#include "tauri-runtime-qt/src/qt.rs.h"

#include <QtCore/QByteArray>
#include <QtCore/QObject>
#include <QtCore/QString>
#include <QtCore/QUrl>
#include <QtGui/QColor>
#include <QtPrintSupport/QPrintDialog>
#include <QtPrintSupport/QPrinter>
#include <QtWebEngineCore/QWebEnginePage>
#include <QtWebEngineCore/QWebEngineScript>
#include <QtWebEngineCore/QWebEngineScriptCollection>
#include <QtWebEngineWidgets/QWebEngineView>
#include <QtWidgets/QDialog>

#include <cstddef>
#include <unordered_map>

namespace {

struct Entry {
    QWebEngineView* view = nullptr;
    QWebEngineView* devtools_view = nullptr;
    QWebEnginePage* devtools_page = nullptr;
};

std::unordered_map<uint64_t, Entry> g_webviews;
uint64_t g_next_id = 1;
uint64_t g_next_script_id = 1;

Entry* find_entry(uint64_t id) {
    auto it = g_webviews.find(id);
    return it == g_webviews.end() ? nullptr : &it->second;
}

QString to_qstring(rust::Str s) {
    return QString::fromUtf8(s.data(), static_cast<qsizetype>(s.size()));
}

/// Emits a page event to Rust.
/// Kinds (must match the bridge documentation in `qt.rs`):
///   1 = page started loading (url)
///   2 = page finished loading (url) -- emitted for success and failure
///   3 = url changed (url)
///   4 = title changed (title)
///   5 = load failed (url) -- additionally emitted when loadFinished(false)
void emit_event(uint64_t id, int kind, const QString& payload) {
    const QByteArray utf8 = payload.toUtf8();
    qt_on_webview_event(id, kind,
                        rust::Str(utf8.constData(), static_cast<std::size_t>(utf8.size())));
}

} // namespace

uint64_t qt_webview_create(uint64_t window_id, rust::Str url) {
    // The view is created unparented; the window agent embeds it through
    // qt_window_attach_webview(window_id, qt_webview_native_handle(id)).
    (void)window_id;

    auto* view = new QWebEngineView();
    const uint64_t id = g_next_id++;

    Entry entry;
    entry.view = view;
    g_webviews.emplace(id, entry);

    QWebEnginePage* page = view->page();
    QObject::connect(page, &QWebEnginePage::loadStarted, view,
                     [id, view]() { emit_event(id, 1, view->url().toString()); });
    QObject::connect(page, &QWebEnginePage::loadFinished, view, [id, view](bool ok) {
        emit_event(id, 2, view->url().toString());
        if (!ok) {
            emit_event(id, 5, view->url().toString());
        }
    });
    QObject::connect(page, &QWebEnginePage::urlChanged, view,
                     [id](const QUrl& u) { emit_event(id, 3, u.toString()); });
    QObject::connect(page, &QWebEnginePage::titleChanged, view,
                     [id](const QString& t) { emit_event(id, 4, t); });

    // Let the scheme layer find this webview's profile for handler install, and
    // install the handlers before the first navigation: a `tauri://localhost`
    // load issued earlier would be rejected for having no scheme handler.
    qt_scheme_bind_webview(id, view);
    qt_scheme_install_handler(id);

    // Backstop: if the widget is destroyed behind our back, drop the handle.
    QObject::connect(view, &QObject::destroyed, view, [id]() { g_webviews.erase(id); });

    if (url.size() != 0) {
        view->load(QUrl(to_qstring(url)));
    }
    return id;
}

uint64_t qt_webview_native_handle(uint64_t id) {
    Entry* e = find_entry(id);
    return e != nullptr ? reinterpret_cast<uint64_t>(e->view) : 0;
}

void qt_webview_navigate(uint64_t id, rust::Str url) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->load(QUrl(to_qstring(url)));
}

rust::String qt_webview_url(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return rust::String();
    }
    return rust::String(e->view->url().toString().toStdString());
}

void qt_webview_reload(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->reload();
}

void qt_webview_eval(uint64_t id, rust::Str script) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    // Default worldId = 0 (MainWorld), fire-and-forget (no result callback).
    e->view->page()->runJavaScript(to_qstring(script));
}

void qt_webview_add_init_script(uint64_t id, rust::Str script, bool main_frame_only) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    // Per-page collection; consumed by the next navigation, so this must run
    // before the first load() for tauri's bootstrap scripts to see the page.
    QWebEngineScript s;
    s.setName(QStringLiteral("tauri_init_%1").arg(g_next_script_id++));
    s.setSourceCode(to_qstring(script));
    s.setInjectionPoint(QWebEngineScript::DocumentCreation);
    s.setWorldId(QWebEngineScript::MainWorld);
    s.setRunsOnSubFrames(!main_frame_only);
    e->view->page()->scripts().insert(s);
}

void qt_webview_set_zoom(uint64_t id, double factor) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->page()->setZoomFactor(static_cast<qreal>(factor));
}

void qt_webview_set_background_color(uint64_t id, int r, int g, int b, int a) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->page()->setBackgroundColor(QColor(r, g, b, a));
}

void qt_webview_show(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->show();
    e->view->raise();
}

void qt_webview_hide(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->hide();
}

void qt_webview_set_bounds(uint64_t id, int x, int y, int w, int h) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    e->view->setGeometry(x, y, w, h);
}

void qt_webview_open_devtools(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->devtools_view != nullptr) {
        return;
    }

    // Qt's only inspector mechanism: bind a second page as the devtools page of
    // the inspected page; binding navigates it to the DevTools frontend.
    QWebEnginePage* page = e->view->page();
    auto* dev_page = new QWebEnginePage(page->profile(), e->view);
    auto* dev_view = new QWebEngineView(dev_page);
    page->setDevToolsPage(dev_page);
    dev_view->resize(1000, 720);
    dev_view->show();

    e->devtools_page = dev_page;
    e->devtools_view = dev_view;

    QObject::connect(dev_view, &QObject::destroyed, dev_view, [id]() {
        Entry* cur = find_entry(id);
        if (cur != nullptr) {
            cur->devtools_view = nullptr;
            cur->devtools_page = nullptr;
        }
    });
}

void qt_webview_close_devtools(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    if (e->view != nullptr && e->view->page() != nullptr) {
        // Unbinds the devtools page (clears its inspected page too).
        e->view->page()->setDevToolsPage(nullptr);
    }
    if (e->devtools_view != nullptr) {
        e->devtools_view->hide();
        e->devtools_view->deleteLater();
        e->devtools_view = nullptr;
    }
    if (e->devtools_page != nullptr) {
        // Dev page is parented to the main view; delete it after its view.
        e->devtools_page->deleteLater();
        e->devtools_page = nullptr;
    }
}

bool qt_webview_is_devtools_open(uint64_t id) {
    Entry* e = find_entry(id);
    return e != nullptr && e->devtools_view != nullptr && e->devtools_view->isVisible();
}

void qt_webview_print(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr) {
        return;
    }
    // Qt 6.11 has no print->dialog API; emulate wry with QPrinter +
    // QPrintDialog and print the page on accept. Requires user interaction.
    QPrinter printer(QPrinter::HighResolution);
    QPrintDialog dialog(&printer, e->view);
    if (dialog.exec() == QDialog::Accepted) {
        e->view->print(&printer);
    }
}

void qt_webview_close(uint64_t id) {
    auto it = g_webviews.find(id);
    if (it == g_webviews.end()) {
        return;
    }
    Entry e = it->second;
    g_webviews.erase(it);

    if (e.view != nullptr && e.view->page() != nullptr) {
        e.view->page()->setDevToolsPage(nullptr);
    }
    if (e.devtools_view != nullptr) {
        e.devtools_view->hide();
        e.devtools_view->deleteLater();
    }
    if (e.devtools_page != nullptr) {
        e.devtools_page->deleteLater();
    }
    if (e.view != nullptr) {
        // QWebEngineView owns the page it created in its default constructor.
        e.view->hide();
        e.view->deleteLater();
    }
}
