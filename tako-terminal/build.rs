// Build scripts inherit the workspace `unsafe_code = "deny"` lint, but
// cxx-qt-build 0.9's only API for passing a C++ compiler flag is the
// `unsafe cc_builder` closure. The relaxation is scoped to this build script.
#![allow(unsafe_code)]

use cxx_qt_build::{CppFile, CxxQtBuilder};
use std::path::{Path, PathBuf};
use std::process::Command;

const GHOSTTY_COMMIT: &str = "b213a72c03b427607b43c89ff4223a7baa079fe8";
const GHOSTTY_TARBALL_SHA256: &str =
    "56654a033fdbed828fd6ac1d275baef920cf1b856ded088c2102e41831a16a0f";
const GHOSTTY_OPTIMIZE: &str = "ReleaseFast";

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let terminal_src = manifest_dir.join("src");
    let ghostty = find_ghostty_artifacts().unwrap_or_else(|| {
        let artifacts = fetch_and_build_ghostty_vt();
        assert!(
            artifacts.include.is_dir() && artifacts.lib.is_file(),
            "could not locate or build libghostty-vt artifacts"
        );
        artifacts
    });
    build_zig_terminal_core(&terminal_src, &ghostty.include);

    unsafe {
        CxxQtBuilder::new()
            .qt_module("Gui")
            .qt_module("Quick")
            .cpp_file(CppFile::from(terminal_src.join("tako_terminal_view.h")))
            .cpp_file(CppFile::from(terminal_src.join("tako_terminal_view.cpp")))
            .include_dir(&ghostty.include)
            .include_dir(&terminal_src)
            .cc_builder(|cc| {
                cc.flag_if_supported("-Wno-sfinae-incomplete");
            })
            .build();
    }

    println!(
        "cargo:rerun-if-changed={}",
        terminal_src.join("core.zig").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        terminal_src.join("tako_terminal_core.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        terminal_src.join("tako_terminal_backend.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        terminal_src.join("tako_terminal_frame.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        terminal_src.join("tako_terminal_view.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        terminal_src.join("tako_terminal_view.cpp").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        ghostty.lib_dir().display()
    );
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    link_pkg_config_libs("freetype2");
    link_pkg_config_libs("harfbuzz");
    println!(
        "cargo:rustc-env=TAKO_TERMINAL_SRC={}",
        terminal_src.display()
    );
    println!(
        "cargo:rustc-env=TAKO_GHOSTTY_INCLUDE={}",
        ghostty.include.display()
    );
    println!(
        "cargo:rustc-env=TAKO_GHOSTTY_LIB_DIR={}",
        ghostty.lib_dir().display()
    );
    println!("cargo:rerun-if-env-changed=TAKO_GHOSTTY_CACHE");
}

fn build_zig_terminal_core(terminal_dir: &Path, ghostty_include: &Path) {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let lib_path = out_dir.join("libtako_terminal_core.a");
    let obj_path = out_dir.join("tako_terminal_core.o");
    let core_zig = terminal_dir.join("core.zig");

    let mut cmd = Command::new("zig");
    cmd.arg("build-obj")
        .arg("-fPIC")
        .arg("-lc")
        .arg("-O")
        .arg("Debug")
        .arg("-I")
        .arg(ghostty_include)
        .arg("-I")
        .arg(terminal_dir)
        .arg(format!("-femit-bin={}", obj_path.display()));
    for flag in pkg_config_flags("freetype2", "--cflags") {
        if flag != "-pthread" {
            cmd.arg(flag);
        }
    }
    for flag in pkg_config_flags("harfbuzz", "--cflags") {
        if flag != "-pthread" {
            cmd.arg(flag);
        }
    }
    let status = cmd
        .arg(&core_zig)
        .status()
        .unwrap_or_else(|e| panic!("failed to run zig build-obj for tako-terminal core: {e}"));
    assert!(
        status.success(),
        "zig build-obj failed for {}",
        core_zig.display()
    );

    let ar = std::env::var_os("AR").unwrap_or_else(|| "ar".into());
    let status = Command::new(&ar)
        .arg("crs")
        .arg(&lib_path)
        .arg(&obj_path)
        .status()
        .unwrap_or_else(|e| panic!("failed to run ar for tako-terminal core archive: {e}"));
    assert!(
        status.success(),
        "ar failed to package {}",
        lib_path.display()
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=tako_terminal_core");
}

fn pkg_config_flags(package: &str, mode: &str) -> Vec<String> {
    let output = Command::new("pkg-config")
        .arg(mode)
        .arg(package)
        .output()
        .unwrap_or_else(|e| panic!("failed to run pkg-config for {package}: {e}"));
    assert!(
        output.status.success(),
        "pkg-config {mode} failed for {package}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn link_pkg_config_libs(package: &str) {
    for flag in pkg_config_flags(package, "--libs") {
        if let Some(path) = flag.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if let Some(lib) = flag.strip_prefix("-l") {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}

struct GhosttyArtifacts {
    src: PathBuf,
    include: PathBuf,
    lib: PathBuf,
}

impl GhosttyArtifacts {
    fn lib_dir(&self) -> &Path {
        self.lib
            .parent()
            .expect("libghostty-vt.a path should have a parent directory")
    }
}

fn find_ghostty_artifacts() -> Option<GhosttyArtifacts> {
    for cache in ghostty_cache_roots() {
        let src = cache
            .join(format!("{GHOSTTY_COMMIT}-{GHOSTTY_OPTIMIZE}"))
            .join("src");
        let artifacts = ghostty_artifacts(src);
        if artifacts.include.is_dir() && artifacts.lib.is_file() {
            return Some(artifacts);
        }
    }
    None
}

fn fetch_and_build_ghostty_vt() -> GhosttyArtifacts {
    let cache = ghostty_cache_roots()
        .into_iter()
        .next()
        .expect("HOME or TAKO_GHOSTTY_CACHE must be set")
        .join(format!("{GHOSTTY_COMMIT}-{GHOSTTY_OPTIMIZE}"));
    let src = cache.join("src");
    let artifacts = ghostty_artifacts(src);

    let tarball = cache.join("ghostty.tar.gz");
    download_tarball(&tarball);
    verify_sha256(&tarball);
    extract_tarball(&tarball, &artifacts.src);
    if !artifacts.lib.is_file() {
        build_libghostty_vt(&artifacts.src);
    }
    artifacts
}

fn ghostty_artifacts(src: PathBuf) -> GhosttyArtifacts {
    GhosttyArtifacts {
        include: src.join("include"),
        lib: src.join("zig-out").join("lib").join("libghostty-vt.a"),
        src,
    }
}

fn ghostty_cache_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("TAKO_GHOSTTY_CACHE") {
        roots.push(PathBuf::from(dir));
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        roots.push(PathBuf::from(xdg).join("tako").join("ghostty-vt"));
    }
    if let Ok(home) = std::env::var("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".cache")
                .join("tako")
                .join("ghostty-vt"),
        );
    }
    roots
}

fn download_tarball(dest: &Path) {
    if dest.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).expect("failed to create ghostty cache dir");
    }
    let url = format!("https://github.com/ghostty-org/ghostty/archive/{GHOSTTY_COMMIT}.tar.gz");
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", &url, "-o"])
        .arg(dest)
        .status()
        .unwrap_or_else(|e| panic!("failed to invoke curl: {e}"));
    assert!(status.success(), "curl failed to download {url}");
}

fn verify_sha256(file: &Path) {
    let out = Command::new("sha256sum")
        .arg(file)
        .output()
        .unwrap_or_else(|e| panic!("failed to invoke sha256sum: {e}"));
    assert!(out.status.success(), "sha256sum failed");
    let sum = String::from_utf8(out.stdout)
        .unwrap_or_else(|e| panic!("sha256sum produced non-UTF8 output: {e}"));
    let actual = sum
        .split_whitespace()
        .next()
        .expect("empty sha256sum output");
    assert!(
        actual == GHOSTTY_TARBALL_SHA256,
        "tarball sha256 mismatch: expected {GHOSTTY_TARBALL_SHA256}, got {actual}"
    );
}

fn extract_tarball(tarball: &Path, dest: &Path) {
    if dest.exists()
        && dest
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        return;
    }
    std::fs::create_dir_all(dest).expect("failed to create ghostty extract dir");
    let status = Command::new("tar")
        .args(["-xf"])
        .arg(tarball)
        .args(["-C"])
        .arg(dest)
        .args(["--strip-components=1"])
        .status()
        .unwrap_or_else(|e| panic!("failed to invoke tar: {e}"));
    assert!(
        status.success(),
        "tar failed to extract {}",
        tarball.display()
    );
}

fn build_libghostty_vt(src: &Path) {
    let status = Command::new("zig")
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={GHOSTTY_OPTIMIZE}"))
        .current_dir(src)
        .status()
        .unwrap_or_else(|e| panic!("failed to invoke `zig` (is zig installed via mise?): {e}"));
    assert!(
        status.success(),
        "`zig build -Demit-lib-vt -Doptimize={GHOSTTY_OPTIMIZE}` failed in {}",
        src.display()
    );
}
