//! `oy run` and `oy chat` compatibility argument types.

use clap::Args;
use std::path::PathBuf;

use crate::config;

#[derive(Debug, Args, Clone)]
pub(super) struct SharedModeArgs {
    #[arg(
        long,
        alias = "agent",
        default_value = "default",
        help = "Safety mode (default: balanced): plan, ask, edit, or auto"
    )]
    pub(super) mode: config::SafetyMode,
    #[arg(
        long = "continue-session",
        default_value_t = false,
        help = "Resume the most recent OpenCode session"
    )]
    pub(super) continue_session: bool,
    #[arg(
        long,
        default_value = "",
        value_name = "SESSION_ID",
        help = "Resume an OpenCode session id"
    )]
    pub(super) resume: String,
}

#[derive(Debug, Args, Clone)]
pub(super) struct RunArgs {
    #[command(flatten)]
    pub(super) shared: SharedModeArgs,
    #[arg(
        long,
        value_name = "PATH",
        help = "Write OpenCode stdout to a workspace file"
    )]
    pub(super) out: Option<PathBuf>,
    #[arg(
        value_name = "PROMPT",
        help = "Task prompt; omitted means read stdin or launch OpenCode in a TTY"
    )]
    pub(super) task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
pub(super) struct ChatArgs {
    #[command(flatten)]
    pub(super) shared: SharedModeArgs,
}
