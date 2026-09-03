//! # oy
//!
//! `oy` prepares deterministic repository evidence for audits and code-quality reviews, and
//! finalizes the resulting Markdown/SARIF reports. The review, audit, and one-finding-fix
//! workflows run as [Agent Skills](https://agentskills.io/) inside whichever agent the user
//! prefers — OpenCode, Cursor, Codex, Copilot, or Gemini CLI all discover skills under
//! `.agents/skills`. `oy setup` installs the skills; the agent executes them under its own
//! permission model. oy does not store provider credentials.
//! The native CLI supports Linux; Windows users should run it in WSL2.
//!
//! ## Start with the CLI
//!
//! The command-line interface is the supported automation surface:
//!
//! ```text
//! oy setup                    # install the oy skills under ~/.agents/skills
//! oy setup --workspace        # install project-local skills under .agents/skills
//! oy audit prepare --path .   # prepare deterministic audit evidence
//! oy audit finalize --run <id># write ISSUES.md or SARIF
//! oy review prepare main      # prepare a git-diff review
//! oy review finalize --run <id># write REVIEW.md
//! oy doctor --check           # verify the skills installation
//! ```
//!
//! See the [getting-started guide](https://oy.adonm.dev/getting-started.html),
//! [workflow guide](https://oy.adonm.dev/workflows.html), and
//! [CLI reference](https://oy.adonm.dev/reference.html) for the user-facing contract.
//!
//! ## Determinism boundary
//!
//! Input collection, ordering, limits, and report rendering are deterministic. Findings are
//! produced by the model the user's agent runs and are not deterministic. The collector also
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
mod review;
mod skills;
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
/// Normal command statuses are returned as `Ok(code)`. Setup, filesystem, and process
/// failures are returned as errors. This function may update the agent skills installation
/// or launch child processes depending on the arguments.
///
/// Prefer invoking the `oy` executable when process isolation or concurrent invocations
/// matter; CLI output configuration is process-global.
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
