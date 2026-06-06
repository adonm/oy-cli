//! Workspace path helpers for deterministic MCP tools.

mod output;
mod paths;
mod read;
mod sloc;

pub(super) const MAX_WORKSPACE_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub(super) use paths::resolve_read_path;
pub(super) use read::read_file_content;
pub(super) use sloc::tool_sloc;
