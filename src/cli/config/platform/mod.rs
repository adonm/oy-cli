//! Platform-specific helpers.
//!
//! Each platform module is gated behind its `#[cfg]` attribute
//! so callers use `cfg`-guarded dispatch at the call site.

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) use windows::restrict_to_owner;
