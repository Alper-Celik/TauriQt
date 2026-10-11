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

#include <cstdio>


// The cxx bridge declares the Rust callback `qt_scheme_request` and the
// `::QtSchemeResponse` struct (with the correct linkage). Never hand-declare
// them here; include the generated header instead.
#include "tauri-runtime-qt/src/qt.rs.h"

#include <cstdio>

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

// A request whose response Tauri will deliver asynchronously (a later call to
// `qt_scheme_respond`). Keyed by the request id passed to `qt_scheme_request`.
struct PendingRequest {
    QPointer<QWebEngineUrlRequestJob> job;
    std::uint64_t webview_id = 0;
};

QHash<std::uint64_t, PendingRequest>& pendingRequests() {
    static QHash<std::uint64_t, PendingRequest> requests;
    return requests;
}

std::uint64_t nextRequestId() {
    static std::uint64_t next = 0;
    return ++next;
}

// Process-wide QObject on the GUI thread used as the receiver context for
// marshalling async responses. Created on the GUI thread (first call is from
// `requestStarted`).
QObject* dispatchContext() {
    static QObject* context = nullptr;
    if (context == nullptr) {
        context = new QObject();
    }
    return context;
}

// Applies a complete response to a job: every header through
// `setAdditionalResponseHeaders`, `Content-Type` (or `application/json`) as the
// reply mime, and the body through a `QBuffer` parented to the job so the web
// engine's cross-thread reads stay valid and nothing leaks.
void applyResponse(QWebEngineUrlRequestJob* job, std::int32_t status,
                   const QByteArray& rawHeaders, const QByteArray& body) {
    if (job == nullptr) {
        return;
    }
    const QList<QPair<QByteArray, QByteArray>> headers = parseHeaderList(rawHeaders);

    QByteArray contentType;
    QByteArray location;
    bool hasExposeHeaders = false;
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
        } else if (header.first.compare("Location", Qt::CaseInsensitive) == 0) {
            location = header.second;
        }
        additional.insert(header.first, header.second);
    }

    // Qt's job API has no HTTP status setter; a redirect is the only
    // status-driven behaviour we can honour. Everything else (including Tauri's
    // 500 error responses) is delivered as a body + headers.
    if (status >= 300 && status < 400 && !location.isEmpty()) {
        job->redirect(QUrl(QString::fromUtf8(location)));
        return;
    }

    // Tauri's JS reads `Tauri-Response` (and keys off `Content-Type`); both must
    // be exposed across the CORS boundary.
    if (!hasExposeHeaders) {
        additional.insert(QByteArrayLiteral("Access-Control-Expose-Headers"),
                          QByteArrayLiteral("Tauri-Response, Content-Type"));
    }

    auto* device = new QBuffer(job); // parented to the job: freed with it
    device->setData(body);
    device->open(QIODevice::ReadOnly);

    job->setAdditionalResponseHeaders(additional);
    job->reply(contentType.isEmpty() ? QByteArrayLiteral("application/json") : contentType, device);
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
// reply `QIODevice` is likewise parented to the job (see `applyResponse`): the
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

    // Collects the full request body before dispatching. `job->requestBody()`
    // is a sequential device whose data may arrive after `requestStarted`, so
    // the body is accumulated on `readyRead` and the dispatch is driven by
    // `readChannelFinished` (the reliable "body complete" signal), never with a
    // partial body.
    void start() {
        QIODevice* body = job_ ? job_->requestBody() : nullptr;
        // Only POST carries a body Tauri reads; every other method (the
        // `tauri://`/`asset://` GET loads, OPTIONS preflights) can be dispatched
        // right away.
        if (body == nullptr || method_ != QByteArrayLiteral("POST")) {
            if (body != nullptr) {
                bodyData_ += body->readAll();
            }
            dispatch();
            return;
        }

        body_ = body;
        bodyData_ += body_->readAll();

        // Dump whatever arrived into the accumulator; never dispatch a partial
        // body here (only the end-of-stream cases below may).
        QObject::connect(body_, &QIODevice::readyRead, this, [this]() {
            if (dispatched_ || body_ == nullptr) {
                return;
            }
            bodyData_ += body_->readAll();
            if (body_->atEnd() || (body_->bytesAvailable() == 0 && !body_->isSequential())) {
                dispatch();
            }
        });
        // Qt's "input stream closed" signal: for the custom-scheme POST body
        // device this fires once the whole body is available.
        QObject::connect(body_, &QIODevice::readChannelFinished, this, [this]() {
            if (dispatched_ || body_ == nullptr) {
                return;
            }
            bodyData_ += body_->readAll();
            dispatch();
        });

        // Already fully buffered when we looked (Chromium usually delivers
        // custom-scheme POST bodies this way): dispatch right away.
        if (!body_->isSequential() && (body_->atEnd() || body_->bytesAvailable() == 0)) {
            dispatch();
            return;
        }

        // QtWebEngine 6.11 hands over an already-ended, empty device for
        // custom-scheme POSTs (the invoke payload travels in the
        // `X-Tauriqt-Payload` header instead), so an empty device dispatches at
        // once; non-empty devices are accumulated to completion.
        if (body_->atEnd() || (body_->bytesAvailable() == 0 && !body_->isSequential())) {
            dispatch();
            return;
        }

        QObject::connect(body_, &QIODevice::readyRead, this, [this]() {
            if (dispatched_ || body_ == nullptr) {
                return;
            }
            bodyData_ += body_->readAll();
            if (body_->atEnd() || (body_->bytesAvailable() == 0 && !body_->isSequential())) {
                dispatch();
            }
        });
        QObject::connect(body_, &QIODevice::readChannelFinished, this, [this]() {
            if (dispatched_ || body_ == nullptr) {
                return;
            }
            bodyData_ += body_->readAll();
            dispatch();
        });
        // Safety net: never leave a request unanswered.
        QMetaObject::invokeMethod(this, [this]() {
            if (!dispatched_ && bodyData_.isEmpty()) {
                dispatch();
            }
        }, Qt::QueuedConnection);
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

        const std::uint64_t request_id = nextRequestId();
        const QByteArray urlBytes = url_.toString(QUrl::FullyEncoded).toUtf8();
        const ::QtSchemeResponse response = qt_scheme_request(
            webview_id_,
            request_id,
            kind_,
            rust::Str(method_.constData(), static_cast<std::size_t>(method_.size())),
            rust::Str(urlBytes.constData(), static_cast<std::size_t>(urlBytes.size())),
            rust::Str(headers_.constData(), static_cast<std::size_t>(headers_.size())),
            rust::Str(bodyData_.constData(), static_cast<std::size_t>(bodyData_.size())));

        if (response.handled == 2) {
            // Tauri answered asynchronously: keep the job alive and wait for a
            // later `qt_scheme_respond(request_id, ...)`. Drop the entry if the
            // job is destroyed first (aborted request).
            PendingRequest pending;
            pending.job = job_;
            pending.webview_id = webview_id_;
            pendingRequests().insert(request_id, pending);
            QObject::connect(job_, &QObject::destroyed, dispatchContext(),
                             [request_id]() { pendingRequests().remove(request_id); });
            return;
        }
        if (response.handled != 1) {
            job_->fail(QWebEngineUrlRequestJob::UrlNotFound);
            return;
        }
        applyResponse(job_, response.status,
                      QByteArray(response.headers.data(),
                                 static_cast<int>(response.headers.size())),
                      QByteArray(response.body.data(), static_cast<int>(response.body.size())));
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
    (void)dispatchContext(); // created here, on the GUI thread
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

void qt_scheme_respond(std::uint64_t request_id, std::int32_t status,
                       rust::Str headers, rust::Str body) {
    // Copy out of the Rust-borrowed strings before marshalling (the borrowed
    // memory is not valid once this call returns).
    const QByteArray rawHeaders(headers.data(), static_cast<int>(headers.size()));
    const QByteArray rawBody(body.data(), static_cast<int>(body.size()));

    // The responder may fire on any thread; always deliver on the GUI thread,
    // where the job and its reply device live.
    QMetaObject::invokeMethod(
        dispatchContext(),
        [request_id, status, rawHeaders, rawBody]() {
            const auto it = pendingRequests().find(request_id);
            if (it == pendingRequests().end()) {
                return; // already answered, or the job was destroyed
            }
            QWebEngineUrlRequestJob* job = it.value().job.data();
            pendingRequests().erase(it);
            if (job == nullptr) {
                return;
            }
            applyResponse(job, status, rawHeaders, rawBody);
        },
        Qt::QueuedConnection);
}
