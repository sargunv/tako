//! Embeddable Qt Quick terminal component.
//!
//! This crate is the Rust-side packaging shim for the C++/Zig `TerminalView`
//! component while Cargo still builds the Tako app. The app consumes this crate
//! only for linkage and one-time QML type registration; terminal behavior lives
//! in `tako_terminal_view.*` and `core.zig`.

#![allow(unsafe_code)]
// The folded libghostty-vt helper modules are test-only while the C++/Zig
// boundary takes ownership of production terminal behavior.
#![allow(dead_code, unused_imports)]
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

pub(crate) mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub(crate) mod frame_ffi {
    include!(concat!(env!("OUT_DIR"), "/frame_bindings.rs"));
}

#[cfg(test)]
pub(crate) mod effects;
#[cfg(test)]
pub(crate) mod gesture;
#[cfg(test)]
pub(crate) mod grid_ref;
#[cfg(test)]
pub(crate) mod input;
#[cfg(test)]
pub(crate) mod modes;
#[cfg(test)]
pub(crate) mod point;
#[cfg(test)]
pub(crate) mod selection;
pub(crate) mod snapshot;
#[cfg(test)]
pub(crate) mod terminal;

#[cfg(test)]
use core::fmt;

/// A failed libghostty-vt call. Wraps the raw `GhosttyResult` code.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Error {
    pub code: ffi::GhosttyResult,
}

#[cfg(test)]
impl Error {
    /// Assert-style helper: turns a non-`GHOSTTY_SUCCESS` result into `Err`.
    pub fn from_result(code: ffi::GhosttyResult) -> Result<(), Self> {
        if code == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Ok(())
        } else {
            Err(Self { code })
        }
    }
}

#[cfg(test)]
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "libghostty-vt error code {}", self.code)
    }
}

#[cfg(test)]
impl std::error::Error for Error {}

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
mod build_smoke {
    use crate::ffi;

    #[test]
    fn ffi_links_and_build_info_succeeds() {
        let mut value: usize = 0;
        let result = unsafe {
            ffi::ghostty_build_info(
                ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MAJOR,
                &mut value as *mut usize as *mut core::ffi::c_void,
            )
        };
        assert_eq!(
            result,
            ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "ghostty_build_info call failed - libghostty-vt link is broken"
        );
    }
}

#[cfg(test)]
mod snapshot_tests;
