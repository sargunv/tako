use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(QmlModule::new("org.tako").qml_file("qml/main.qml"))
        .qt_module("Gui")
        .qt_module("Quick")
        .files(["src/qobject.rs"])
        .build();
}
