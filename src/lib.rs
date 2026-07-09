//! # oy
//!
//! `oy` adds repeatable repository audit and review workflows to
//! [opencode](https://opencode.ai/). Its local MCP server provides deterministic repository
//! collection, ordered chunks, target-diff input, and normalized Markdown/SARIF reports.
//! opencode remains responsible for model execution, providers, sessions, permissions, and
//! general coding tools.
//!
//! ## Start with the CLI
//!
//! The command-line interface is the supported automation surface:
//!
//! ```text
//! oy setup --dry-run       # preview generated opencode integration
//! oy setup                 # install the global integration
//! oy audit                 # write ISSUES.md
//! oy review main           # write REVIEW.md for git diff main
//! oy enhance <finding-id>  # remediate one reported finding
//! ```
//!
//! See the [getting-started guide](https://adonm.github.io/oy-cli/getting-started.html),
//! [workflow guide](https://adonm.github.io/oy-cli/workflows.html), and
//! [CLI/MCP reference](https://adonm.github.io/oy-cli/reference.html) for the user-facing
//! contract.
//!
//! ## Determinism boundary
//!
//! Input collection, ordering, limits, and report rendering are deterministic. Findings are
//! produced by the model selected in opencode and are not deterministic. The collector also
//! has documented exclusions; “all chunks” does not mean every byte in a repository.
//!
//! ## Rust API
//!
//! This crate exists primarily to keep the `oy` binary entrypoint small. [`run`] and
//! [`err_line`] are public for that entrypoint and lightweight embedding, but spawning the
//! `oy` executable is preferred for automation. Other modules and implementation details are
//! private and may change without a semver-stable library API commitment.
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! // Arguments exclude the executable name, just like std::env::args().skip(1).
//! let exit_code = oy::run(vec!["doctor".into(), "--json".into()]).await?;
//! assert_eq!(exit_code, 0);
//! # Ok(())
//! # }
//! ```

#![recursion_limit = "256"]

mod audit;
mod cli;
mod mcp;
mod opencode;
mod review;
mod tools;

pub(crate) use cli::{config, ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextDecodeError {
    Binary,
    NonUtf8,
}

pub(crate) fn decode_utf8(raw: Vec<u8>) -> Result<String, TextDecodeError> {
    if raw.contains(&0) {
        return Err(TextDecodeError::Binary);
    }
    String::from_utf8(raw).map_err(|_| TextDecodeError::NonUtf8)
}

/// Runs the `oy` command dispatcher with arguments that exclude the executable name.
///
/// Normal command and delegated opencode exit statuses are returned as `Ok(code)`. Setup,
/// filesystem, process-launch, and protocol failures are returned as errors. This function may
/// update opencode integration files or launch child processes depending on the arguments.
///
/// Prefer invoking the `oy` executable when process isolation or concurrent invocations matter;
/// CLI output configuration is process-global.
pub async fn run(argv: Vec<String>) -> anyhow::Result<i32> {
    cli::app::run(argv).await
}

/// Writes a formatted diagnostic line to standard error.
///
/// This is primarily exposed for the binary entrypoint.
///
/// ```
/// oy::err_line(format_args!("error: {}", "example"));
/// ```
pub fn err_line(args: std::fmt::Arguments<'_>) {
    ui::err_line(args);
}
