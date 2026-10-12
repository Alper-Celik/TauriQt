// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Cookie store access for the Qt runtime (mirroring, writes, deletion, clear).
//
// THREADING: every function here runs on the Qt GUI thread (the thread that
// created the QApplication). `QWebEngineView` / `QWebEnginePage` /
// `QWebEngineProfile` / `QWebEngineCookieStore` are GUI-thread-affine, and the
// Rust side only calls these from the dispatchers tauri invokes on that thread.
// The cookie signals are documented as *not* a place for heavy work (they block
// Chromium's IO thread while a blocking connection runs); the forwarding below
// is a few map lookups plus one Rust call each, so it stays cheap.
//
// HANDLE CONTRACT: `webview_ptr` is the value returned by
// `qt_webview_native_handle()` — the address of a `QWebEngineView` created by
// `qt_webview_create()`. It is cast back with `reinterpret_cast`. A null or
// unknown pointer makes every function here a no-op (the caller decided the
// handle is valid when it registered the view; the C++ side must not throw
// inside webview creation).
//
// WHAT QT REPORTS (verified against Qt 6.11.2 / its Chromium 140):
//   * `cookieAdded` / `cookieRemoved` carry the *canonical* cookie Chromium
//     stored: a host-only cookie keeps its bare host as the domain, a domain
//     cookie keeps the leading dot (".example.com"), and the path is never
//     empty ("/" at minimum).
//   * `QWebEngineCookieStore::setCookie()` takes a `QNetworkCookie` and turns it
//     into a `Set-Cookie` line (`QNetworkCookie::toRawForm()`), which Chromium
//     parses against a URL it synthesises from the cookie's own domain — so a
//     write works from any page scheme, including the runtime's custom
//     `tauri://` scheme. A *dotless* domain attribute is made a domain cookie by
//     Chromium (it prepends the dot) unless the host has no registrable domain
//     (`localhost`, IP literals), in which case it stays host-only.
//   * `QWebEngineCookieStore::deleteCookie()` builds Chromium's deletion filter
//     from the cookie *name* plus a URL derived from the domain — the cookie's
//     path is NOT part of the filter, so a delete removes every path variant of
//     that name for the host.
//   * Qt 6.11 never fills `QNetworkCookie::sameSitePolicy()` when converting a
//     stored cookie back for a signal (`type_conversion.h` drops it), so the
//     callbacks below always report `Default` (0) for events. The mapping is
//     still implemented for the values the enum defines.
//   * `loadAllCookies()` emits one `cookieAdded` per stored cookie and never
//     signals completion — which is exactly why the Rust side keeps a mirror.

#include "qt_cookies.hpp"

// cxx puts the `extern "Rust"` callbacks declared in the bridge module into the
// generated header; never hand-declare them here.
#include "tauri-runtime-qt/src/bridge/cookies.rs.h"

#include <QtCore/QByteArray>
#include <QtCore/QDateTime>
#include <QtCore/QObject>
#include <QtCore/QString>
#include <QtCore/QUrl>
#include <QtNetwork/QNetworkCookie>
#include <QtWebEngineCore/QWebEngineCookieStore>
#include <QtWebEngineCore/QWebEnginePage>
#include <QtWebEngineCore/QWebEngineProfile>
#include <QtWebEngineWidgets/QWebEngineView>

#include <cstddef>
#include <cstdint>
#include <unordered_map>
#include <vector>

namespace {

/// Views whose stores are mirrored, by view pointer.
std::unordered_map<std::uint64_t, QWebEngineCookieStore*> g_view_stores;
/// Every view watching a store. A key's presence means the store's signals are
/// connected (the vector may be empty after a view is destroyed).
std::unordered_map<QWebEngineCookieStore*, std::vector<std::uint64_t>> g_store_views;

/// `QNetworkCookie` value -> the bridge's `same_site` code.
std::int32_t same_site_code(QNetworkCookie::SameSite policy) {
    switch (policy) {
    case QNetworkCookie::SameSite::None:
        return 1;
    case QNetworkCookie::SameSite::Lax:
        return 2;
    case QNetworkCookie::SameSite::Strict:
        return 3;
    case QNetworkCookie::SameSite::Default:
        break;
    }
    return 0;
}

/// `rust::Str` view over a byte array that stays alive for the call.
rust::Str str_of(const QByteArray& bytes) {
    return rust::Str(bytes.constData(), static_cast<std::size_t>(bytes.size()));
}

/// `rust::Str` bytes as a `QByteArray` (both are UTF-8 by contract).
QByteArray to_qbytearray(rust::Str s) {
    return QByteArray(s.data(), static_cast<qsizetype>(s.size()));
}

QString to_qstring(rust::Str s) {
    return QString::fromUtf8(s.data(), static_cast<qsizetype>(s.size()));
}

QNetworkCookie::SameSite same_site_from_code(std::int32_t code) {
    switch (code) {
    case 1:
        return QNetworkCookie::SameSite::None;
    case 2:
        return QNetworkCookie::SameSite::Lax;
    case 3:
        return QNetworkCookie::SameSite::Strict;
    default:
        return QNetworkCookie::SameSite::Default;
    }
}

QWebEngineCookieStore* store_of(QWebEngineView* view) {
    if (view == nullptr || view->page() == nullptr) {
        return nullptr;
    }
    QWebEngineProfile* profile = view->page()->profile();
    return profile != nullptr ? profile->cookieStore() : nullptr;
}

QWebEngineView* view_of(std::uint64_t webview_ptr) {
    return reinterpret_cast<QWebEngineView*>(webview_ptr);
}

/// Forwards one store event to every Rust mirror watching that store.
void forward_cookie(QWebEngineCookieStore* store, const QNetworkCookie& cookie, bool removed) {
    auto it = g_store_views.find(store);
    if (it == g_store_views.end() || it->second.empty()) {
        return;
    }
    const QByteArray name = cookie.name();
    const QByteArray domain = cookie.domain().toUtf8();
    const QByteArray path = cookie.path().toUtf8();

    if (removed) {
        for (const std::uint64_t view_ptr : it->second) {
            qt_on_cookie_removed(view_ptr, str_of(name), str_of(domain), str_of(path));
        }
        return;
    }

    const QByteArray value = cookie.value();
    const bool secure = cookie.isSecure();
    const bool http_only = cookie.isHttpOnly();
    const std::int32_t same_site = same_site_code(cookie.sameSitePolicy());
    const std::int64_t expires_ms = cookie.isSessionCookie()
        ? -1
        : static_cast<std::int64_t>(cookie.expirationDate().toMSecsSinceEpoch());
    for (const std::uint64_t view_ptr : it->second) {
        qt_on_cookie_added(view_ptr, str_of(name), str_of(value), str_of(domain), str_of(path),
                           secure, http_only, same_site, expires_ms);
    }
}

} // namespace

void qt_cookies_watch(std::uint64_t webview_ptr) {
    QWebEngineView* view = view_of(webview_ptr);
    if (view == nullptr || g_view_stores.count(webview_ptr) != 0) {
        return; // unknown handle, or already watching this pointer
    }
    QWebEngineCookieStore* store = store_of(view);
    if (store == nullptr) {
        return;
    }

    g_view_stores.emplace(webview_ptr, store);
    std::vector<std::uint64_t>& watchers = g_store_views[store];
    const bool first_watcher = watchers.empty();
    watchers.push_back(webview_ptr);

    // One connection per store: the default profile's store is shared by every
    // view the runtime creates, so connecting per view would deliver every
    // event once per connected view.
    if (first_watcher) {
        QObject::connect(store, &QWebEngineCookieStore::cookieAdded, store,
                         [store](const QNetworkCookie& cookie) {
                             forward_cookie(store, cookie, false);
                         });
        QObject::connect(store, &QWebEngineCookieStore::cookieRemoved, store,
                         [store](const QNetworkCookie& cookie) {
                             forward_cookie(store, cookie, true);
                         });
    }

    // Drop the registration with the view so its address can be recycled by a
    // later view without leaking events into the old mirror. No context object:
    // the functor must run *during* the view's destruction (it only touches this
    // file's registries, never the dying view).
    QObject::connect(view, &QObject::destroyed, [webview_ptr]() {
        auto entry = g_view_stores.find(webview_ptr);
        if (entry == g_view_stores.end()) {
            return;
        }
        QWebEngineCookieStore* store = entry->second;
        g_view_stores.erase(entry);
        auto watchers = g_store_views.find(store);
        if (watchers == g_store_views.end()) {
            return;
        }
        std::vector<std::uint64_t>& list = watchers->second;
        for (auto it = list.begin(); it != list.end(); ++it) {
            if (*it == webview_ptr) {
                list.erase(it);
                break;
            }
        }
    });

    // Deterministic fill: emits `cookieAdded` for every cookie the store holds
    // (no completion signal, hence the Rust mirror). Repeating it for a second
    // view on the same store is harmless — the mirror is keyed by
    // name/domain/path and treats an add as an upsert.
    store->loadAllCookies();
}

void qt_cookies_set(std::uint64_t webview_ptr,
                    rust::Str name,
                    rust::Str value,
                    rust::Str domain,
                    rust::Str path,
                    bool secure,
                    bool http_only,
                    std::int32_t same_site,
                    std::int64_t expires_ms) {
    QWebEngineView* view = view_of(webview_ptr);
    QWebEngineCookieStore* store = store_of(view);
    if (store == nullptr) {
        return;
    }

    QNetworkCookie cookie(to_qbytearray(name), to_qbytearray(value));
    const QString domain_str = to_qstring(domain);
    const QString path_str = to_qstring(path);
    if (!path_str.isEmpty()) {
        cookie.setPath(path_str);
    }
    cookie.setSecure(secure);
    cookie.setHttpOnly(http_only);
    if (same_site != 0) {
        cookie.setSameSitePolicy(same_site_from_code(same_site));
    }
    if (expires_ms >= 0) {
        // `expires_ms == -1` (session cookie) leaves the date invalid, which is
        // what `isSessionCookie()` and `toRawForm()` key on.
        cookie.setExpirationDate(QDateTime::fromMSecsSinceEpoch(expires_ms));
    }

    if (!domain_str.isEmpty()) {
        // Domain variant. `setCookie()` without an origin lets Chromium build
        // the source URL from `domain + path`, which is what makes the write
        // valid from a custom-scheme page. Chromium normalises this to
        // ".domain" (domain cookie) when the host has a registrable domain, and
        // keeps it bare (host-only) for `localhost` / IP literals — the Rust
        // mirror normalises its optimistic key the same way.
        cookie.setDomain(domain_str);
        store->setCookie(cookie);
        return;
    }

    // Host-only variant: no domain attribute, the origin URL decides the host.
    // (A cookie with neither a domain nor an origin serialises to a URL like
    // "http:///" and is dropped by Chromium, so the view's host is used.)
    const QUrl page_url = view->url();
    if (page_url.host().isEmpty()) {
        return;
    }
    QUrl origin;
    origin.setScheme(secure ? QStringLiteral("https") : QStringLiteral("http"));
    origin.setHost(page_url.host());
    origin.setPath(QStringLiteral("/"));
    store->setCookie(cookie, origin);
}

void qt_cookies_delete(std::uint64_t webview_ptr,
                       rust::Str name,
                       rust::Str domain,
                       rust::Str path) {
    QWebEngineCookieStore* store = store_of(view_of(webview_ptr));
    if (store == nullptr) {
        return;
    }
    QNetworkCookie cookie(to_qbytearray(name), QByteArray());
    const QString domain_str = to_qstring(domain);
    if (!domain_str.isEmpty()) {
        // Chromium's deletion filter is built from the name plus the URL
        // synthesised from the domain; the path does not restrict the filter.
        cookie.setDomain(domain_str);
    }
    const QString path_str = to_qstring(path);
    if (!path_str.isEmpty()) {
        cookie.setPath(path_str);
    }
    store->deleteCookie(cookie);
}

void qt_cookies_clear_all(std::uint64_t webview_ptr) {
    QWebEngineView* view = view_of(webview_ptr);
    QWebEngineCookieStore* store = store_of(view);
    if (store == nullptr) {
        return;
    }
    store->deleteAllCookies();
    QWebEngineProfile* profile = view->page()->profile();
    if (profile != nullptr) {
        profile->clearHttpCache();
    }
    // NOT cleared (no API for it on a live profile): localStorage /
    // sessionStorage, IndexedDB, service-worker registrations and Cache
    // Storage. All of them live under the profile's storage directory, which
    // this runtime does not delete while the profile is alive. Visited links
    // are cleared by `qt_webview_clear_browsing_data()` (the W3 webview op),
    // not here: this function is the cookie/HTTP-cache half.
}
