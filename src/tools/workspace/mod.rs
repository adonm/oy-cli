//! Workspace path helpers for deterministic MCP tools.

mod outline;
mod output;
mod paths;
mod sloc;

pub(super) use outline::{has_universal_ctags, tool_outline};
pub(super) use sloc::has_tokei;
pub(super) use sloc::tool_sloc;
