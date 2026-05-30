//! Workspace filesystem tools and path trust boundary.
//!
//! Listing, reading, searching, line counting, replacement, and patching all
//! validate paths against the configured workspace before touching the host.

mod diff;
mod discovery;
mod list;
mod output;
mod patch;
mod paths;
mod read;
mod replace;
mod search;
mod sloc;

pub(super) const MAX_WORKSPACE_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub(super) use list::tool_list;
pub(super) use patch::tool_patch;
pub(super) use paths::{repos_cache_root, resolve_read_path};
pub(super) use read::{read_file_content, tool_read, tool_read_multiple_files};
pub(super) use replace::tool_replace;
pub(super) use search::tool_search;
pub(super) use sloc::tool_sloc;
