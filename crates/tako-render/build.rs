// Generate the C header for the `tako_render` C ABI surface (FramePlan, Vertex,
// and the `tako_surface_*` / `tako_gl_renderer_*` extern fns). The C++
// `TakoTerminalView` includes it instead of re-declaring those types by hand,
// so a field change can't silently desync the two sides.
//
// We tried propagating the path to downstream build scripts via
// `cargo:cbindgen_dir` + `DEP_TAKO_RENDER_*`, but cargo didn't expose that
// metadata for this path dependency (cxx's `links`-declared metadata
// propagated fine; tako-render's didn't). Writing into the source tree next to
// the hand-written C++ headers is robust to that and lets `tako_terminal_view`
// resolve `#include "tako_render.h"` in its own directory. The file is
// gitignored and content-compared so it only rewrites (and re-triggers C++
// recompile) when the contract actually changes.

use std::path::PathBuf;

const HEADER_REL: &str = "cpp/tako_render.h";

fn main() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let header_path = crate_dir.join(HEADER_REL);

    let config = cbindgen::Config {
        language: cbindgen::Language::Cxx,
        // The generated header carries its own doc comments; skip the cbindgen
        // version banner so the file is stable across regenerations.
        include_version: false,
        // Don't autogen `#include <cstdint>` etc. — the C++ TU that includes
        // this already pulls in the Qt/std headers it needs.
        no_includes: true,
        sys_includes: vec![
            "cstdint".to_string(), // uint8_t, uint32_t, uint64_t
            "cstddef".to_string(), // size_t / uintptr_t
        ],
        ..cbindgen::Config::default()
    };

    let mut generated = Vec::new();
    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
        .expect("cbindgen: failed to parse tako-render")
        .write(&mut generated);
    let generated = String::from_utf8(generated).expect("cbindgen output not UTF-8");

    // Only touch the file when content actually changed, so unchanged
    // regenerations don't bump the mtime and force a C++ recompile.
    let prev = std::fs::read_to_string(&header_path).unwrap_or_default();
    if prev != generated {
        if let Some(parent) = header_path.parent() {
            std::fs::create_dir_all(parent).expect("create cpp/ dir");
        }
        std::fs::write(&header_path, generated).expect("write tako_render.h");
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/frame_planner.rs");
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=src/qml_init.rs");
    // If the C++ side edits the generated header by hand (it shouldn't), let
    // the next build notice and regenerate.
    println!("cargo:rerun-if-changed={}", HEADER_REL);
}
