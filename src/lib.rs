//! Internal library crate for the `oy` binary.
//!
//! The supported automation surface is the `oy` command-line interface. Rust module paths
//! and exported items beyond [`run`] and [`err_line`] are intentionally unstable while the
//! binary is still evolving.

#![recursion_limit = "256"]

mod agent;
mod audit;
mod cli;
mod llm;
mod tools;

pub(crate) use agent::{compaction, model, session};
pub(crate) use cli::{chat, config, ui};

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

/// Runs the `oy` command dispatcher.
pub async fn run(argv: Vec<String>) -> anyhow::Result<i32> {
    cli::app::run(argv).await
}

/// Writes a formatted diagnostic line to standard error.
pub fn err_line(args: std::fmt::Arguments<'_>) {
    ui::err_line(args);
}
