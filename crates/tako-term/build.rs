// Builds libghostty-vt from a pinned upstream ghostty source tree and generates
// Rust bindings via bindgen. See ROADMAP.md §2.2 and Phase 0 §2.
//
// Flow: download + verify the pinned commit tarball (cached under
// $XDG_CACHE_HOME/tako/ghostty-vt/<commit>/), run `zig build -Demit-lib-vt`,
// then bindgen `include/ghostty/vt.h`. The heavy Zig build is cached so repeat
// cargo builds skip it. Linux-only (shells out to curl/tar/sha256sum).

use std::path::PathBuf;
use std::process::Command;

// ghostty-org/ghostty upstream `main` HEAD as of Phase 0 start. The latest
// stable tag (v1.3.1) lacks the full libghostty-vt C API the ROADMAP binds
// (no render.h/terminal.h/build_info.h, no static-lib build), so we pin the
// upstream commit that introduced them. vt.h is marked unstable — bump this
// deliberately and re-verify the bindgen surface each time.
const GHOSTTY_COMMIT: &str = "b213a72c03b427607b43c89ff4223a7baa079fe8";
const GHOSTTY_TARBALL_SHA256: &str =
    "56654a033fdbed828fd6ac1d275baef920cf1b856ded088c2102e41831a16a0f";

fn main() {
    let cache = cache_dir().join(GHOSTTY_COMMIT);
    let src = cache.join("src");
    let lib = src.join("zig-out").join("lib").join("libghostty-vt.a");
    let include = src.join("include");

    if !lib.exists() {
        let tarball = cache.join("ghostty.tar.gz");
        download_tarball(&tarball);
        verify_sha256(&tarball);
        extract_tarball(&tarball, &src);
        build_libghostty_vt(&src);
        assert!(lib.exists(), "zig build did not produce {lib:?}");
    }

    let header = include.join("ghostty").join("vt.h");
    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy().into_owned())
        .clang_arg(format!("-I{}", include.display()))
        .derive_default(true)
        .generate_comments(false)
        .generate()
        .expect("bindgen failed to generate libghostty-vt bindings");

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    bindings
        .write_to_file(out.join("bindings.rs"))
        .expect("failed to write bindings.rs");

    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!(
        "cargo:rustc-link-search=native={}",
        src.join("zig-out").join("lib").display()
    );
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TAKO_GHOSTTY_CACHE");
}

fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TAKO_GHOSTTY_CACHE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").expect("HOME not set");
        format!("{home}/.cache")
    });
    PathBuf::from(base).join("tako").join("ghostty-vt")
}

fn download_tarball(dest: &PathBuf) {
    if dest.exists() {
        return;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).expect("failed to create cache dir");
    }
    let url = format!("https://github.com/ghostty-org/ghostty/archive/{GHOSTTY_COMMIT}.tar.gz");
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", &url, "-o"])
        .arg(dest)
        .status()
        .unwrap_or_else(|e| panic!("failed to invoke curl: {e}"));
    assert!(status.success(), "curl failed to download {url}");
}

fn verify_sha256(file: &PathBuf) {
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

fn extract_tarball(tarball: &PathBuf, dest: &PathBuf) {
    if dest.exists()
        && dest
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        return;
    }
    std::fs::create_dir_all(dest).expect("failed to create extract dir");
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

fn build_libghostty_vt(src: &PathBuf) {
    let status = Command::new("zig")
        .args(["build", "-Demit-lib-vt"])
        .current_dir(src)
        .status()
        .unwrap_or_else(|e| {
            panic!("failed to invoke `zig` (is core:zig installed via mise?): {e}")
        });
    assert!(
        status.success(),
        "`zig build -Demit-lib-vt` failed in {}",
        src.display()
    );
}
