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

pub mod pty;
pub mod snapshot;
pub mod terminal;

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

use ffi::{GhosttyBuildInfo, GhosttyResult, ghostty_build_info};

/// Query a numeric `ghostty_build_info` field. Returns the result code and the
/// out-value (unchanged on non-success). Used by the Phase 0 §2 smoke test to
/// prove the bindgen + link path.
pub fn build_info_usize(field: GhosttyBuildInfo) -> (GhosttyResult, usize) {
    let mut value: usize = 0;
    // SAFETY: `ghostty_build_info` with a numeric out-kind writes a `usize`
    // (see ghostty/example/c-vt-build-info). The pointer is to a local.
    let result =
        unsafe { ghostty_build_info(field, &mut value as *mut usize as *mut core::ffi::c_void) };
    (result, value)
}

/// libghostty-vt major version, or `None` if the build reports none.
pub fn version_major() -> Option<usize> {
    let (result, value) = build_info_usize(ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MAJOR);
    (result == ffi::GhosttyResult_GHOSTTY_SUCCESS).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 0 §2 smoke test: the bindgen bindings resolve and libghostty-vt.a
    /// links. We assert the FFI call succeeds — the actual version depends on
    /// the pinned ghostty build's injected metadata.
    #[test]
    fn ffi_links_and_build_info_succeeds() {
        let (result, _) = build_info_usize(ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MAJOR);
        assert_eq!(
            result,
            ffi::GhosttyResult_GHOSTTY_SUCCESS,
            "ghostty_build_info call failed — libghostty-vt link is broken"
        );
    }

    #[test]
    fn version_major_is_reported() {
        // A successful build should report a major version. We don't pin the
        // exact value (it tracks the ghostty commit), only that it is set.
        assert!(
            version_major().is_some(),
            "no version reported; got {:?}",
            version_major()
        );
    }
}
