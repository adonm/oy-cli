//! Workspace path helpers for deterministic MCP tools.

mod outline;
mod output;
mod paths;
mod sighthound;
mod sloc;

pub(super) use outline::{has_universal_ctags, tool_outline};
pub(super) use sighthound::{has_sighthound, tool_sighthound};
pub(super) use sloc::has_tokei;
pub(super) use sloc::tool_sloc;
