// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT
//
// Custom URL schemes (`ipc://`, `tauri://`, `asset://`) for the Qt runtime.
//
// The runtime implements no protocol logic of its own: every request is
// serialized into the `\n`-separated `k: v` form Tauri expects and handed to the
// Rust callback `qt_scheme_request` (declared by the cxx bridge), whose
// `QtSchemeResponse` is then written straight back onto the Qt job.

#include "qt_scheme.hpp"

// The cxx bridge declares the Rust callback `qt_scheme_request` and the
// `::QtSchemeResponse` struct (with the correct linkage). Never hand-declare
// them here; include the generated header instead.
#include "tauri-runtime-qt/src/qt.rs.h"

#include <QtCore/QBuffer>
#include <QtCore/QByteArray>
#include <QtCore/QDebug>
#include <QtCore/QHash>
#include <QtCore/QIODevice>
#include <QtCore/QList>
#include <QtCore/QMultiMap>
#include <QtCore/QPair>
#include <QtCore/QPointer>
#include <QtCore/QUrl>
#include <QtWebEngineCore/QWebEnginePage>
#include <QtWebEngineCore/QWebEngineProfile>
#include <QtWebEngineCore/QWebEngineUrlRequestJob>
#include <QtWebEngineCore/QWebEngineUrlScheme>
#include <QtWebEngineCore/QWebEngineUrlSchemeHandler>
#include <QtWebEngineWidgets/QWebEngineView>

namespace {

// Scheme kinds shared with the Rust side: 0 = ipc, 1 = tauri, 2 = asset.
enum SchemeKind : std::int32_t {
    SchemeIpc = 0,
    SchemeTauri = 1,
    SchemeAsset = 2,
};

// webview id -> backing view (weak).
QHash<std::uint64_t, QPointer<QWebEngineView>>& webviewRegistry() {
    static QHash<std::uint64_t, QPointer<QWebEngineView>> registry;
    return registry;
}

// Parses a `\n`-separated `k: v` header list. Splits on the first `:` and
// strips surrounding whitespace and any trailing CR from each value.
QList<QPair<QByteArray, QByteArray>> parseHeaderList(const QByteArray& blob) {
    QList<QPair<QByteArray, QByteArray>> headers;
    const QList<QByteArray> lines = blob.split('\n');
    for (const QByteArray& raw : lines) {
        QByteArray line = raw;
        if (line.endsWith('\r')) {
            line.chop(1);
        }
        if (line.trimmed().isEmpty()) {
            continue;
        }
        const int colon = line.indexOf(':');
        if (colon < 0) {
            continue;
        }
        const QByteArray key = line.left(colon).trimmed();
        if (key.isEmpty()) {
            continue;
        }
        headers.append({key, line.mid(colon + 1).trimmed()});
    }
    return headers;
}

// Serializes the job's request headers into the `\n`-separated `k: v` form Tauri
// expects. Tauri's ipc parser hard-requires an `Origin` header, which
// `requestHeaders()` may omit, so it is synthesized from the job's initiator
// (the serialized origin of the content that started the request).
QByteArray serializeRequestHeaders(QWebEngineUrlRequestJob* job) {
    QByteArray out;
    bool hasOrigin = false;
    const QMap<QByteArray, QByteArray> headers = job->requestHeaders();
    for (auto it = headers.constBegin(); it != headers.constEnd(); ++it) {
        if (it.key().compare("Origin", Qt::CaseInsensitive) == 0) {
            hasOrigin = true;
        }
        out += it.key();
        out += ": ";
        out += it.value();
        out += '\n';
    }
    if (!hasOrigin) {
        const QUrl initiator = job->initiator();
        if (initiator.isValid() && !initiator.isEmpty()) {
            out += "Origin: ";
            out += initiator.toString(QUrl::FullyEncoded).toUtf8();
            out += '\n';
        }
    }
    return out;
}

// One `QWebEngineUrlSchemeHandler` per scheme; forwards every request to Rust.
class TauriSchemeHandler final : public QWebEngineUrlSchemeHandler {
public:
    TauriSchemeHandler(std::int32_t kind, std::uint64_t webview_id, QObject* parent = nullptr)
        : QWebEngineUrlSchemeHandler(parent), kind_(kind), webview_id_(webview_id) {}

    void requestStarted(QWebEngineUrlRequestJob* job) override;

private:
    std::int32_t kind_;
    std::uint64_t webview_id_;
};

// Owns the per-request state and drives the reply.
//
// Created on the GUI thread from `requestStarted` and parented to the job, so
// `job->destroyed` tears it down and any still-queued work becomes a no-op. The
// reply `QIODevice` is likewise parented to the job (see `writeResponse`): the
// web engine may read the device on another thread until the job is deleted, and
// a job child is exactly as long-lived as the job.
class JobContext final : public QObject {
public:
    JobContext(QWebEngineUrlRequestJob* job, std::int32_t kind, std::uint64_t webview_id)
        : QObject(job), job_(job), kind_(kind), webview_id_(webview_id) {
        method_ = job->requestMethod();
        url_ = job->requestUrl();
        headers_ = serializeRequestHeaders(job);
    }

    // Reads whatever request body is already available and dispatches. If the
    // body device only delivers data via `readyRead`, accumulate it and dispatch
    // once the device has drained; a zero-delay fallback guarantees the request
    // is never left unanswered (an empty body is valid for Tauri).
    void start() {
        QIODevice* body = job_ ? job_->requestBody() : nullptr;
        // Only POST carries a body Tauri reads; every other method (the
        // `tauri://`/`asset://` GET loads, OPTIONS preflights) can be dispatched
        // right away without waiting on the body device.
        if (body == nullptr || method_ != QByteArrayLiteral("POST")) {
            if (body != nullptr) {
                bodyData_ += body->readAll();
            }
            dispatch();
            return;
        }
        body_ = body;
        if (body_->bytesAvailable() > 0) {
            bodyData_ += body_->readAll();
            dispatch();
            return;
        }
        QObject::connect(body_, &QIODevice::readyRead, this, [this]() {
            if (body_ == nullptr || dispatched_) {
                return;
            }
            const QByteArray chunk = body_->readAll();
            if (chunk.isEmpty()) {
                return;
            }
            bodyData_ += chunk;
            if (body_->bytesAvailable() == 0) {
                dispatch();
            }
        });
        QMetaObject::invokeMethod(this, [this]() { dispatch(); }, Qt::QueuedConnection);
    }

private:
    void dispatch() {
        if (dispatched_) {
            return;
        }
        dispatched_ = true;
        if (!job_) {
            return;
        }

        const QByteArray urlBytes = url_.toString(QUrl::FullyEncoded).toUtf8();
        const ::QtSchemeResponse response = qt_scheme_request(
            webview_id_,
            kind_,
            rust::Str(method_.constData(), static_cast<std::size_t>(method_.size())),
            rust::Str(urlBytes.constData(), static_cast<std::size_t>(urlBytes.size())),
            rust::Str(headers_.constData(), static_cast<std::size_t>(headers_.size())),
            rust::Str(bodyData_.constData(), static_cast<std::size_t>(bodyData_.size())));

        if (response.handled != 1) {
            job_->fail(QWebEngineUrlRequestJob::UrlNotFound);
            return;
        }
        writeResponse(response);
    }

    void writeResponse(const ::QtSchemeResponse& response) {
        if (!job_) {
            return;
        }
        const QByteArray rawHeaders(response.headers.data(),
                                    static_cast<int>(response.headers.size()));
        const QList<QPair<QByteArray, QByteArray>> headers = parseHeaderList(rawHeaders);

        QByteArray contentType;
        QByteArray location;
        bool hasExposeHeaders = false;
        bool hasTauriResponse = false;
        QMultiMap<QByteArray, QByteArray> additional;

        for (const QPair<QByteArray, QByteArray>& header : headers) {
            if (header.first.compare("Content-Type", Qt::CaseInsensitive) == 0) {
                if (contentType.isEmpty()) {
                    contentType = header.second;
                }
                continue; // passed to reply() instead
            }
            if (header.first.compare("Access-Control-Expose-Headers", Qt::CaseInsensitive) == 0) {
                hasExposeHeaders = true;
            } else if (header.first.compare("Tauri-Response", Qt::CaseInsensitive) == 0) {
                hasTauriResponse = true;
            } else if (header.first.compare("Location", Qt::CaseInsensitive) == 0) {
                location = header.second;
            }
            additional.insert(header.first, header.second);
        }

        // Qt's job API has no HTTP status setter; a redirect is the only
        // status-driven behaviour we can honour. Everything else (including
        // Tauri's 500 error responses) is delivered as a body + headers.
        if (response.status >= 300 && response.status < 400 && !location.isEmpty()) {
            job_->redirect(QUrl(QString::fromUtf8(location)));
            return;
        }

        // Tauri's JS reads `Tauri-Response` to pick callback vs error; make sure
        // it is exposed across the CORS boundary.
        if (hasTauriResponse && !hasExposeHeaders) {
            additional.insert(QByteArrayLiteral("Access-Control-Expose-Headers"),
                              QByteArrayLiteral("Tauri-Response"));
        }

        const QByteArray body(response.body.data(), static_cast<int>(response.body.size()));
        auto* device = new QBuffer(job_); // parented to the job: freed with it
        device->setData(body);
        device->open(QIODevice::ReadOnly);

        job_->setAdditionalResponseHeaders(additional);
        job_->reply(contentType.isEmpty() ? QByteArrayLiteral("application/octet-stream") : contentType,
                    device);
    }

    QPointer<QWebEngineUrlRequestJob> job_;
    std::int32_t kind_;
    std::uint64_t webview_id_;
    QByteArray method_;
    QUrl url_;
    QByteArray headers_;
    QByteArray bodyData_;
    QIODevice* body_ = nullptr; // owned by the job
    bool dispatched_ = false;
};

void TauriSchemeHandler::requestStarted(QWebEngineUrlRequestJob* job) {
    if (job == nullptr) {
        return;
    }
    (new JobContext(job, kind_, webview_id_))->start();
}

void installOnProfile(QWebEngineProfile* profile, std::uint64_t webview_id) {
    if (profile == nullptr) {
        return;
    }
    if (profile->urlSchemeHandler(QByteArrayLiteral("ipc")) != nullptr) {
        return; // already installed for this profile
    }
    profile->installUrlSchemeHandler(QByteArrayLiteral("ipc"),
                                     new TauriSchemeHandler(SchemeIpc, webview_id, profile));
    profile->installUrlSchemeHandler(QByteArrayLiteral("tauri"),
                                     new TauriSchemeHandler(SchemeTauri, webview_id, profile));
    profile->installUrlSchemeHandler(QByteArrayLiteral("asset"),
                                     new TauriSchemeHandler(SchemeAsset, webview_id, profile));
}

} // namespace

void qt_scheme_register_schemes() {
    static bool registered = false;
    if (registered) {
        return;
    }
    registered = true;

    // Tauri's URLs are `{scheme}://localhost/{path}`: host present, no port, so
    // Syntax::Host with PortUnspecified is the exact fit. SecureScheme makes the
    // origin trustworthy, CorsEnabled enables cross-origin loads from the page,
    // and FetchApiAllowed is required for `fetch()`/XHR with a body.
    const QByteArray schemes[] = {
        QByteArrayLiteral("ipc"),
        QByteArrayLiteral("tauri"),
        QByteArrayLiteral("asset"),
    };
    for (const QByteArray& name : schemes) {
        QWebEngineUrlScheme scheme(name);
        scheme.setSyntax(QWebEngineUrlScheme::Syntax::Host);
        scheme.setDefaultPort(QWebEngineUrlScheme::PortUnspecified);
        scheme.setFlags(QWebEngineUrlScheme::SecureScheme
                        | QWebEngineUrlScheme::CorsEnabled
                        | QWebEngineUrlScheme::FetchApiAllowed);
        QWebEngineUrlScheme::registerScheme(scheme);
    }
}

void qt_scheme_bind_webview(std::uint64_t webview_id, QWebEngineView* view) {
    webviewRegistry().insert(webview_id, QPointer<QWebEngineView>(view));
}

void qt_scheme_install_handler(std::uint64_t webview_id) {
    const auto it = webviewRegistry().constFind(webview_id);
    QWebEngineView* view = (it != webviewRegistry().constEnd()) ? it.value().data() : nullptr;
    if (view == nullptr) {
        qWarning("tauri-runtime-qt: qt_scheme_install_handler(%llu): webview not bound; "
                 "call qt_scheme_bind_webview() after creating the view",
                 static_cast<unsigned long long>(webview_id));
        return;
    }
    QWebEnginePage* page = view->page();
    installOnProfile(page != nullptr ? page->profile() : nullptr, webview_id);
}
