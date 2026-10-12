// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

#include "qt_app.hpp"
#include "qt_scheme.hpp"

#include <QtCore/QCoreApplication>
#include <QtCore/QString>
#include <QtWidgets/QApplication>

namespace {
QApplication* g_app = nullptr;
}

int qt_init(int argc) {
    if (g_app != nullptr || QCoreApplication::instance() != nullptr) {
        return 1;
    }
    QCoreApplication::setAttribute(Qt::AA_ShareOpenGLContexts);

    // Tauri's URL schemes must be registered before any GUI object exists.
    qt_scheme_register_schemes();

    static int argc_storage = argc > 0 ? argc : 1;
    static char argv0[] = "tauri-runtime-qt";
    static char* argv[] = {argv0, nullptr};
    g_app = new QApplication(argc_storage, argv);

    return 0;
}

int qt_run() {
    if (g_app == nullptr) {
        return 1;
    }
    return QApplication::exec();
}

rust::String qt_version() {
    return rust::String(qVersion());
}
