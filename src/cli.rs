//! Command parsing, dispatch, configuration, terminal UI, and the
//! interactive chat shell.
//!
//! This module owns the CLI surface — argument parsing, subcommand
//! handlers, config paths, safety modes, rendering, and the REPL.

pub(crate) mod app;
pub(crate) mod config;
pub(crate) mod ui;
