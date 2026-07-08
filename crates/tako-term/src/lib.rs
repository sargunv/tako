//! libghostty-vt bindgen wrapper, PTY bridge, OSC dispatch.
//!
//! See ROADMAP.md §2.3 and §4. Bindings for the pinned upstream
//! ghostty-org/ghostty `libghostty-vt` C API are generated at build time by
//! `build.rs` (see `OUT_DIR/bindings.rs`).
//!
//! The safe surface for Phase 0 §3 lives in [`terminal`], [`snapshot`], and
//! [`pty`]: spawn a PTY, feed bytes into a [`terminal::Terminal`], snapshot a
//! [`snapshot::FrameSnapshot`] from a [`terminal::RenderState`], then hand the
//! snapshot to a renderer.

// bindgen emits raw FFI (extern "C", pointers, unsafe fns) and unidiomatic C
// names. The unsafe relaxation is crate-scoped; the workspace `unsafe_code =
// deny` still governs every other crate.
#![allow(unsafe_code)]
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub mod effects;
pub mod input;
pub mod modes;
pub mod mouse;
pub mod pty;
pub mod snapshot;
pub mod terminal;

pub mod key;

use core::fmt;

/// A failed libghostty-vt call. Wraps the raw `GhosttyResult` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    pub code: ffi::GhosttyResult,
}

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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "libghostty-vt error code {}", self.code)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod build_smoke {
    use crate::ffi;

    /// Phase 0 §2 smoke test: the bindgen bindings resolve, libghostty-vt.a
    /// links, and `ghostty_build_info` succeeds. The actual version value
    /// tracks the pinned ghostty commit's injected metadata (and may be 0 at
    /// commits that haven't baked one in), so we assert success, not a value.
    #[test]
    fn ffi_links_and_build_info_succeeds() {
        let mut value: usize = 0;
        // SAFETY: numeric out-kind writes a `usize` to the local out-pointer.
        let result = unsafe {
            ffi::ghostty_build_info(
                ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MAJOR,
                &mut value as *mut usize as *mut core::ffi::c_void,
            )
        };
        assert_eq!(
            result,
            ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "ghostty_build_info call failed — libghostty-vt link is broken"
        );
    }
}
