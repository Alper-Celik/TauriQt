// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#include <QApplication>
#include <QUrl>
#include <QWebEngineView>

#include <cxx.h>

inline int run_webview(rust::Str url) {
    QCoreApplication::setAttribute(Qt::AA_ShareOpenGLContexts);

    int argc = 1;
    char argv0[] = "tauriqt";
    char* argv[] = {argv0, nullptr};

    QApplication app(argc, argv);

    QWebEngineView view;
    view.load(QUrl(QString::fromUtf8(url.data(), static_cast<qsizetype>(url.size()))));
    view.resize(1280, 800);
    view.setWindowTitle(QStringLiteral("TauriQt"));
    view.show();

    return QApplication::exec();
}
