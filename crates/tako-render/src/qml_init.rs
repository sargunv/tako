//! One-time QML type registration for hand-written C++ QQuickItem types.
//!
//! cxx-qt-build's compiled `org.tako` module only registers bridge-generated
//! types, so C++ items like `TakoTerminalView` register themselves imperatively
//! via `qmlRegisterType` (see `cpp/tako_terminal_view.cpp`,
//! `tako_register_qml_types`). This module exposes that as a safe Rust call.
//!
//! The workspace denies `unsafe_code`; the extern call is scoped here only.

#![allow(unsafe_code)]

unsafe extern "C" {
    fn tako_register_qml_types();
}

/// Register `TerminalView` (and any future C++ QQuickItem types) with the QML
/// engine. Idempotent. Call once before loading QML.
pub fn register_qml_types() {
    // SAFETY: the function performs only `qmlRegisterType`, which is safe to
    // call from the GUI thread before QML loading.
    unsafe {
        tako_register_qml_types();
    }
}
