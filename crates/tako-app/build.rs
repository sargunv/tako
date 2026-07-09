// Build scripts inherit the workspace `unsafe_code = "deny"` lint, but
// cxx-qt-build 0.9's only API for passing a C++ compiler flag is the
// `unsafe cc_builder` closure. The relaxation is scoped to this build script.
#![allow(unsafe_code)]

use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    unsafe {
        CxxQtBuilder::new_qml_module(QmlModule::new("org.tako").qml_file("qml/main.qml"))
            .qt_module("Gui")
            .qt_module("Quick")
            .cc_builder(|cc| {
                cc.flag_if_supported("-Wno-sfinae-incomplete");
            })
            .build();
    }
}
