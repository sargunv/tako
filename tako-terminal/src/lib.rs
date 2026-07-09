//! Embeddable Qt Quick terminal component.
//!
//! This crate is the Rust-side packaging shim for the C++/Zig `TerminalView`
//! component while Cargo still builds the Tako app. The app consumes this crate
//! only for linkage and one-time QML type registration; terminal behavior lives
//! in `tako_terminal_view.*` and the `*.zig` implementation core.

#![allow(unsafe_code)]

unsafe extern "C" {
    fn tako_register_qml_types();
}

/// Register `TerminalView` with QML. Idempotent; call before loading QML.
pub fn register_qml_types() {
    // SAFETY: the C++ function only calls `qmlRegisterType` for the
    // `org.tako.terminal` module and is intended to run on the GUI thread before
    // QML loading.
    unsafe {
        tako_register_qml_types();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn zig_core_tests_pass() {
        let terminal_src = env!("TAKO_TERMINAL_SRC");
        let ghostty_include = env!("TAKO_GHOSTTY_INCLUDE");
        let ghostty_lib_dir = env!("TAKO_GHOSTTY_LIB_DIR");
        let tests_zig = Path::new(terminal_src).join("tests.zig");

        let mut cmd = Command::new("zig");
        cmd.arg("test")
            .arg(&tests_zig)
            .arg("-lc")
            .arg("-I")
            .arg(ghostty_include)
            .arg("-I")
            .arg(terminal_src)
            .arg("-L")
            .arg(ghostty_lib_dir)
            .arg("-lghostty-vt");

        add_pkg_config_args(&mut cmd, "freetype2", "--cflags");
        add_pkg_config_args(&mut cmd, "harfbuzz", "--cflags");
        add_pkg_config_args(&mut cmd, "freetype2", "--libs");
        add_pkg_config_args(&mut cmd, "harfbuzz", "--libs");

        let status = cmd
            .status()
            .expect("failed to run zig test for tako-terminal core");
        assert!(
            status.success(),
            "zig test failed for {}",
            tests_zig.display()
        );
    }

    fn add_pkg_config_args(cmd: &mut Command, package: &str, mode: &str) {
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
        let stdout = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| panic!("pkg-config returned non-UTF8 output: {e}"));
        for flag in stdout.split_whitespace() {
            if flag == "-pthread" {
                continue;
            }
            cmd.arg(flag);
        }
    }
}
