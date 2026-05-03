//! Library facade for the `oy` command-line application.
//!
//! `oy` is primarily a binary crate: install and run the [`oy` CLI][repo] to inspect,
//! edit, audit, and ask questions about local repositories. The library surface is kept
//! intentionally small for the binary, integration tests, and lightweight embedders that
//! want to invoke the same command handlers.
//!
//! Most implementation modules are private so their internals can evolve without creating
//! an API compatibility promise. If you need to automate `oy`, prefer spawning the `oy`
//! binary and using its documented CLI unless one of the functions below exactly fits your
//! use case.
//!
//! Useful project documentation:
//!
//! - [README](https://github.com/wagov-dtt/oy-cli#readme) for installation and user guide.
//! - [Architecture](https://github.com/wagov-dtt/oy-cli/blob/main/docs/architecture.md)
//!   for runtime flow, module responsibilities, and trust boundaries.
//! - [Tool safety](https://github.com/wagov-dtt/oy-cli/blob/main/docs/tool-safety.md)
//!   for capability and approval-mode details.
//!
//! [repo]: https://github.com/wagov-dtt/oy-cli

#![recursion_limit = "256"]

mod agent;
mod audit;
mod cli;
mod tools;

pub(crate) use agent::{bedrock, compaction, model, session};
pub(crate) use cli::{app, chat, config, ui};

pub use ui::{OutputMode, set_output_mode};

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

/// Runs the `oy` command dispatcher with command-line arguments excluding the program name.
///
/// This is the same application entry point used by the binary after `src/main.rs` strips
/// the executable name. It returns the process exit code that the binary should use.
///
/// Prefer invoking the `oy` binary for automation unless embedding the CLI in-process is
/// specifically needed.
pub async fn run(argv: Vec<String>) -> anyhow::Result<i32> {
    app::run(argv).await
}

/// Returns the interactive chat help text shown by `/help`.
///
/// This helper is public so snapshot tests and lightweight integrations can verify or
/// display the same help content as the interactive shell.
pub fn chat_help_text() -> String {
    chat::chat_help_text()
}

/// Formats a tool result using the same compact preview renderer as the CLI.
///
/// `name` is the tool name, and `value` is the JSON result payload returned by that tool.
/// The output is intended for humans and may change with CLI presentation improvements;
/// do not treat it as a stable machine-readable format.
pub fn preview_tool_output(name: &str, value: &serde_json::Value) -> String {
    tools::preview_tool_output(name, value)
}

/// Writes a formatted diagnostic line to standard error using the current UI output mode.
///
/// Use [`format_args!`] to avoid allocating an intermediate `String`:
///
/// ```no_run
/// oy::err_line(format_args!("provider failed: {}", "timeout"));
/// ```
pub fn err_line(args: std::fmt::Arguments<'_>) {
    ui::err_line(args);
}
