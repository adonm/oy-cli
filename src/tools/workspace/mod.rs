//! Workspace path helpers for deterministic MCP tools.

mod output;
mod paths;
#[cfg(feature = "outline")]
mod read;
mod sloc;

#[cfg(feature = "outline")]
pub(super) const MAX_WORKSPACE_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[cfg(feature = "outline")]
pub(super) use paths::resolve_read_path;
#[cfg(feature = "outline")]
pub(super) use read::read_file_content;
pub(super) use sloc::tool_sloc;
