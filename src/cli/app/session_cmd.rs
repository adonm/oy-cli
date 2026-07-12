//! `oy run` and `oy chat` convenience argument types.

use clap::Args;

#[derive(Debug, Args, Clone)]
pub(super) struct SharedSessionArgs {
    #[arg(
        long = "continue-session",
        conflicts_with = "resume",
        default_value_t = false,
        help = "Resume the most recent session"
    )]
    pub(super) continue_session: bool,
    #[arg(
        long,
        conflicts_with = "continue_session",
        default_value = "",
        value_name = "SESSION_ID",
        help = "Resume a session id"
    )]
    pub(super) resume: String,
}

#[derive(Debug, Args, Clone)]
pub(super) struct RunArgs {
    #[command(flatten)]
    pub(super) shared: SharedSessionArgs,
    #[arg(
        long,
        default_value_t = false,
        help = "Let OpenCode approve permission requests once; explicit denies still apply"
    )]
    pub(super) auto: bool,
    #[arg(
        value_name = "PROMPT",
        help = "Task prompt; omitted means read stdin or launch opencode in a TTY"
    )]
    pub(super) task: Vec<String>,
}

#[derive(Debug, Args, Clone)]
pub(super) struct ChatArgs {
    #[command(flatten)]
    pub(super) shared: SharedSessionArgs,
}
