// Build scripts inherit the workspace `unsafe_code = "deny"` lint, but
// cxx-qt-build 0.9's only API for passing a C++ compiler flag is the
// `unsafe cc_builder` closure (it hands out a raw `&mut cc::Build`). This is a
// standalone single-file build-script binary, not shipped library code, so the
// relaxation is scoped to this file only.
#![allow(unsafe_code)]

use cxx_qt_build::{CppFile, CxxQtBuilder, QmlModule};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    // The TakoTerminalView C++ source lives in tako-render; reference it by
    // absolute path so cxx-qt-build compiles + moc's it into this QML module.
    let render_cpp: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tako-render")
        .join("cpp");

    // The C++ view pulls in <ghostty/vt/key/event.h> + <ghostty/vt/mouse/event.h>
    // for enum constants. Resolve the include root from the tako-term cache.
    let ghostty_include = find_ghostty_include().unwrap_or_else(|| {
        panic!(
            "could not locate libghostty-vt include dir. Set TAKO_GHOSTTY_CACHE or run \
             `cargo build -p tako-term` first to fetch the source tarball."
        )
    });

    // Suppress GCC 16's -Wsfinae-incomplete: libstdc++'s std::data SFINAE probe
    // sees QChar/QRegularExpression forward-declared-but-incomplete before Qt
    // defines them later in the TU. Benign header-ordering artifact between
    // system Qt6 and libstdc++ headers (all in system includes, not our code).
    // flag_if_supported no-ops on non-GCC compilers.
    unsafe {
        CxxQtBuilder::new_qml_module(QmlModule::new("org.tako").qml_file("qml/main.qml"))
            .qt_module("Gui")
            .qt_module("Quick")
            .cpp_file(CppFile::from(render_cpp.join("tako_terminal_view.h")))
            .cpp_file(CppFile::from(render_cpp.join("tako_terminal_view.cpp")))
            .include_dir(&ghostty_include)
            .cc_builder(|cc| {
                cc.flag_if_supported("-Wno-sfinae-incomplete");
            })
            .build();
    }
}

/// Locate the libghostty-vt include directory. Mirrors the cache-key logic in
/// `tako-term/build.rs` so the same commit + optimize mode is used.
fn find_ghostty_include() -> Option<PathBuf> {
    const COMMIT: &str = "b213a72c03b427607b43c89ff4223a7baa079fe8";
    const OPTIMIZE: &str = "ReleaseFast";

    if let Ok(dir) = std::env::var("TAKO_GHOSTTY_CACHE") {
        let p = PathBuf::from(dir)
            .join(format!("{COMMIT}-{OPTIMIZE}"))
            .join("src")
            .join("include");
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        let p = PathBuf::from(xdg)
            .join("tako")
            .join("ghostty-vt")
            .join(format!("{COMMIT}-{OPTIMIZE}"))
            .join("src")
            .join("include");
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("tako")
            .join("ghostty-vt")
            .join(format!("{COMMIT}-{OPTIMIZE}"))
            .join("src")
            .join("include");
        if p.is_dir() {
            return Some(p);
        }
    }
    // Final fallback: ask the user to build tako-term first (which fetches
    // the tarball). Suppress unused-warning if Command import is unused.
    let _ = Command::new("echo");
    None
}
