//! `oy model` convenience argument types.

use clap::Args;

#[derive(Debug, Args, Clone)]
pub(super) struct ModelArgs {
    #[arg(
        value_name = "MODEL",
        help = "Optional model/provider substring filter for the OpenCode 2 model API"
    )]
    pub(super) model: Option<String>,
}
