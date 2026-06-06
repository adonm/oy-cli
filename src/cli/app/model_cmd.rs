//! `oy model` convenience argument types.

use clap::Args;

#[derive(Debug, Args, Clone)]
pub(super) struct ModelArgs {
    #[arg(
        value_name = "MODEL",
        help = "Optional model/provider filter passed to `opencode models`"
    )]
    pub(super) model: Option<String>,
}

#[cfg(test)]
pub(super) fn is_exact_model_spec(value: &str) -> bool {
    let value = value.trim();
    value.contains("::") || value.contains('/') || value.contains(':') || value.contains('.')
}
