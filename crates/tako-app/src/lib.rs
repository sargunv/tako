//! cxx-qt bridge: registers Rust model to QML, main entry.
//!
//! See ROADMAP.md §2.3 and the Phase 0 spike (§12).

// Pull in tako-render so its extern "C" surface symbols (tako_surface_*) are
// linked into the final binary for the C++ TakoTerminalView to call.
use tako_render as _;

use cxx_qt::casting::Upcast;
use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QQmlEngine, QUrl};
use std::pin::Pin;

/// Boot the Qt application: create the GUI, load the QML module, and run the
/// event loop.
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Register hand-written C++ QQuickItem types (TakoTerminalView) before
    // loading QML.
    tako_render::qml_init::register_qml_types();

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/org/tako/qml/main.qml"));
    }

    if let Some(engine) = engine.as_mut() {
        let engine: Pin<&mut QQmlEngine> = engine.upcast_pin();
        engine.on_quit(|_| log::info!("QML quit")).release();
    }

    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
