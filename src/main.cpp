// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#include <QApplication>
#include <QUrl>
#include <QWebEngineView>

int main(int argc, char *argv[]) {
    QApplication app(argc, argv);

    QWebEngineView view;
    view.load(QUrl("https://www.google.com"));
    view.resize(1280, 800);
    view.setWindowTitle("TauriQt");
    view.show();

    return QApplication::exec();
}
