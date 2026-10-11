// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

// Qt-backed webview registry (see qt_webview.hpp for the threading contract:
// every entry point here runs on the Qt GUI thread).
//
// The page-event callback `qt_on_webview_event` comes from the cxx bridge.

#include "qt_webview.hpp"

#include "qt_scheme.hpp"
#include "qt_window.hpp"
#include "tauri-runtime-qt/src/qt.rs.h"
#include "tauri-runtime-qt/src/bridge/webview.rs.h"

#include <QtCore/QByteArray>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonValue>
#include <QtCore/QObject>
#include <QtCore/QString>
#include <QtCore/QUrl>
#include <QtCore/QVariant>
#include <QtGui/QColor>
#include <QtPrintSupport/QPrintDialog>
#include <QtPrintSupport/QPrinter>
#include <QtWebEngineCore/QWebEngineCookieStore>
#include <QtWebEngineCore/QWebEnginePage>
#include <QtWebEngineCore/QWebEngineProfile>
#include <QtWebEngineCore/QWebEngineScript>
#include <QtWebEngineCore/QWebEngineScriptCollection>
#include <QtWebEngineWidgets/QWebEngineView>
#include <QtWidgets/QDialog>
#include <QtWidgets/QLayout>

#include <cmath>
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

/// Serializes a `runJavaScript` completion value to compact JSON text.
///
/// `QJsonDocument::fromVariant()` alone is not enough: it converts only JSON
/// *containers* (`QVariantMap`/`QVariantHash`/`QVariantList`/`QStringList`) and
/// returns a **null document** for every scalar (`QString`, `int`, `bool`, ...;
/// verified against Qt 6.11), which would make every scalar result look like
/// `null`. So the value is wrapped in a one-element array and the brackets are
/// stripped again: a compact JSON array is exactly `[<value>]` with no
/// whitespace, and a value can never contain an unbalanced top-level bracket
/// (JSON escapes them inside strings). This keeps Qt's own escaping.
///
/// JS `undefined` (or anything Qt cannot represent as a `QJsonValue`) is
/// reported as the JSON literal `null`, matching what JSON can express.
QByteArray json_text(const QVariant& value) {
    const QJsonValue json_value = QJsonValue::fromVariant(value);
    if (json_value.isUndefined()) {
        return QByteArrayLiteral("null");
    }
    const QJsonDocument document(QJsonArray{json_value});
    const QByteArray wrapped = document.toJson(QJsonDocument::Compact);
    return wrapped.mid(1, wrapped.size() - 2);
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

/// Takes `view` out of its parent's layout, if it is managed by one.
///
/// GEOMETRY OWNERSHIP (the decision the runtime makes): the window agent's
/// `qt_window_attach_webview()` puts every attached view into the window's
/// zero-margin `QVBoxLayout`, which makes the webview fill the window client
/// area and follow every window resize — the right default for tauri, where a
/// window's webview *is* its client area. `set_bounds` / `set_size` /
/// `set_position`, in contrast, mean "place the webview exactly here", which a
/// layout cannot express (it would immediately re-apply its own geometry), so
/// the first such call detaches the view from the layout and Qt stops managing
/// its geometry. Consequence, documented in `ops/webview.rs`: from then on the
/// webview no longer resizes with its window (tauri's `set_auto_resize` is a
/// no-op on this runtime), and re-attaching the view to a window
/// (`qt_webview_reparent`) puts it back under layout management.
void detach_from_layout(QWebEngineView* view) {
    QWidget* parent = view->parentWidget();
    if (parent == nullptr) {
        return;
    }
    if (QLayout* layout = parent->layout()) {
        layout->removeWidget(view);
    }
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

    // TAURIQT_EVAL_DEBUG smoke test: for the first webview, ask Rust once when
    // the first page load finishes. Rust checks the environment variable and,
    // when it is `1`, runs `document.title = 'eval-ok'` through the regular
    // eval_with_callback path and prints the callback result (see
    // `ops/webview.rs::debug_hook_first_load`). Without the variable this is a
    // single cheap call into Rust per process.
    if (id == 1) {
        QObject::connect(page, &QWebEnginePage::loadFinished, view, [id](bool) {
            static bool fired = false;
            if (!fired) {
                fired = true;
                qt_webview_on_first_load(id);
            }
        });
    }

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

QtWebviewGeometry qt_webview_geometry(uint64_t id) {
    QtWebviewGeometry geometry{};
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr) {
        geometry.ok = false;  // all other fields stay zero
        return geometry;
    }

    // `QWidget::geometry()` is in device-independent pixels; tauri's
    // `Physical*` values are device pixels, so scale by the widget's dpr.
    // `QWidget::devicePixelRatioF()` follows the widget's window, which is the
    // correct scale under Wayland fractional scaling (a `QScreen`'s dpr can
    // differ from the window's there).
    const QRect rect = e->view->geometry();
    const qreal dpr = e->view->devicePixelRatioF();
    geometry.ok = true;
    geometry.x = static_cast<std::int32_t>(std::lround(rect.x() * dpr));
    geometry.y = static_cast<std::int32_t>(std::lround(rect.y() * dpr));
    geometry.width = static_cast<std::int32_t>(std::lround(rect.width() * dpr));
    geometry.height = static_cast<std::int32_t>(std::lround(rect.height() * dpr));
    return geometry;
}

bool qt_webview_set_size(uint64_t id, int width, int height, bool physical) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr) {
        return false;
    }
    detach_from_layout(e->view);
    const qreal dpr = e->view->devicePixelRatioF();
    const int logical_width = physical ? static_cast<int>(std::lround(width / dpr)) : width;
    const int logical_height = physical ? static_cast<int>(std::lround(height / dpr)) : height;
    e->view->resize(logical_width, logical_height);
    return true;
}

bool qt_webview_set_position(uint64_t id, int x, int y, bool physical) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr) {
        return false;
    }
    detach_from_layout(e->view);
    const qreal dpr = e->view->devicePixelRatioF();
    const int logical_x = physical ? static_cast<int>(std::lround(x / dpr)) : x;
    const int logical_y = physical ? static_cast<int>(std::lround(y / dpr)) : y;
    // QWidget::move is relative to the parent widget, i.e. to the window's
    // client-area origin, which is what tauri's webview position means.
    e->view->move(logical_x, logical_y);
    return true;
}

bool qt_webview_set_focus(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr) {
        return false;
    }
    e->view->setFocus();
    return true;
}

bool qt_webview_eval_with_callback(uint64_t id, uint64_t request_id, rust::Str script) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr || e->view->page() == nullptr) {
        return false;
    }

    const QString source = to_qstring(script);
    e->view->page()->runJavaScript(source, [request_id](const QVariant& value) {
        // Qt hands the JS completion value back as a QVariant; `json_text`
        // serializes it as a JSON string so Rust/tauri see the same shape wry
        // produces (an undefined/unrepresentable value becomes `null`).
        const QByteArray json = json_text(value);
        qt_on_eval_result(request_id,
                          rust::Str(json.constData(), static_cast<std::size_t>(json.size())));
    });
    return true;
}

bool qt_webview_reparent(uint64_t id, uint64_t window_native) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr) {
        return false;
    }

    // The window registry's accessors return 0 for an unknown handle, and a
    // window that exists always has a native window id, so `winId() == 0` is a
    // truthful "no such window" probe (there is no `qt_window_widget()` accessor
    // to ask directly).
    if (qt_window_winid(window_native) == 0) {
        return false;
    }

    // The window agent adopts the widget: `qt_window_attach_webview()` reparents
    // it into the target window (dropping it from its previous layout),
    // (re)creates the window's zero-margin layout, adds the view and shows it.
    // That is exactly "reparent into another window", and it puts the view back
    // under layout management (see `detach_from_layout`).
    detach_from_layout(e->view);
    qt_window_attach_webview(
        window_native, static_cast<uint64_t>(reinterpret_cast<quintptr>(e->view)));
    return true;
}

bool qt_webview_clear_browsing_data(uint64_t id) {
    Entry* e = find_entry(id);
    if (e == nullptr || e->view == nullptr || e->view->page() == nullptr) {
        return false;
    }
    QWebEngineProfile* profile = e->view->page()->profile();
    if (profile == nullptr) {
        return false;
    }

    // What Qt 6.11 can clear on a live profile:
    //   * the HTTP cache (in-memory for off-the-record profiles),
    //   * every cookie of the profile's cookie store,
    //   * the visited-links list.
    // Both cache clearing and cookie deletion are asynchronous inside
    // QtWebEngine: `clearHttpCache()` signals `clearHttpCacheCompleted()` and
    // `deleteAllCookies()` has no completion signal at all, so this is
    // fire-and-forget by design (the tauri method returns `Result<()>` and
    // blocking the GUI thread on the GUI thread's own work would deadlock).
    //
    // What it CANNOT clear (no API on a live profile):
    //   * localStorage / sessionStorage,
    //   * IndexedDB databases,
    //   * service-worker registrations and Cache Storage,
    //   * WebSQL is gone in Chromium, and HTTP auth caches are not exposed.
    // Those live under the profile's `persistentStoragePath()`; the only way to
    // drop them is to remove that directory while no profile is alive, which
    // this runtime does not do. Note also that tauri's `clear_all_browsing_data`
    // is documented as clearing "all" browsing data; on this runtime it clears
    // the list above and leaves the listed stores in place.
    profile->clearHttpCache();
    profile->clearAllVisitedLinks();
    if (QWebEngineCookieStore* store = profile->cookieStore()) {
        store->deleteAllCookies();
    }
    return true;
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
