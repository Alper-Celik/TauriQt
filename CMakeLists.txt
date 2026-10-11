# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT

cmake_minimum_required(VERSION 3.21)
project(tauriqt LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Qt6 REQUIRED COMPONENTS Widgets WebEngineWidgets)
qt_standard_project_setup()

add_executable(tauriqt src/main.cpp)
install(TARGETS tauriqt RUNTIME DESTINATION bin)
target_link_libraries(tauriqt PRIVATE Qt6::Widgets Qt6::WebEngineWidgets)
