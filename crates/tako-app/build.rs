use cxx_qt_build::{CppFile, CxxQtBuilder, QmlModule};
use std::path::PathBuf;

fn main() {
    // The TakoTerminalView C++ source lives in tako-render; reference it by
    // absolute path so cxx-qt-build compiles + moc's it into this QML module.
    let render_cpp: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tako-render")
        .join("cpp");

    CxxQtBuilder::new_qml_module(QmlModule::new("org.tako").qml_file("qml/main.qml"))
        .qt_module("Gui")
        .qt_module("Quick")
        .cpp_file(CppFile::from(render_cpp.join("tako_terminal_view.h")))
        .cpp_file(CppFile::from(render_cpp.join("tako_terminal_view.cpp")))
        .files(["src/qobject.rs"])
        .build();
}
