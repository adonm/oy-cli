//! # oy
//!
//! `oy` adds one focused coding agent and repeatable repository audit/review workflows to
//! [OpenCode 2](https://v2.opencode.ai/). File-backed CLI preparation and finalization provide
//! deterministic repository collection, ordered chunks, target-diff input, and normalized
//! Markdown/SARIF reports. OpenCode remains responsible for model execution, providers,
//! authenticated sessions, permissions, and general coding tools. oy does not store provider
//! credentials.
//! The native CLI supports Linux and macOS; Windows users should run it in WSL2.
//!
//! ## Start with the CLI
//!
//! The command-line interface is the supported automation surface:
//!
//! ```text
//! oy setup --dry-run       # preview package/config migration
//! oy setup                 # register the version-matched OpenCode package
//! oy audit                 # write ISSUES.md
//! oy review main           # write REVIEW.md for git diff main
//! oy enhance <finding-id>  # remediate one reported finding
//! ```
//!
//! See the [getting-started guide](https://oy.adonm.dev/getting-started.html),
//! [workflow guide](https://oy.adonm.dev/workflows.html), and
//! [CLI and OpenCode reference](https://oy.adonm.dev/reference.html) for the user-facing
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
//! # fn example() -> anyhow::Result<()> {
//! // Arguments exclude the executable name, just like std::env::args().skip(1).
//! let exit_code = oy::run(vec!["doctor".into(), "--json".into()])?;
//! assert_eq!(exit_code, 0);
//! # Ok(())
//! # }
//! ```

#![recursion_limit = "256"]

mod artifacts;
mod audit;
mod cli;
mod opencode;
mod review;
mod tools;
mod workflow;

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
/// update opencode integration config or launch child processes depending on the arguments.
///
/// Prefer invoking the `oy` executable when process isolation or concurrent invocations matter;
/// CLI output configuration is process-global.
pub fn run(argv: Vec<String>) -> anyhow::Result<i32> {
    cli::app::run(argv)
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
