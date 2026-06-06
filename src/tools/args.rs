use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum ExcludeArg {
    String(String),
    Array(Vec<String>),
}

impl ExcludeArg {
    pub(super) fn patterns(&self) -> Vec<String> {
        match self {
            Self::String(value) => value
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            Self::Array(values) => values
                .iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }
}

fn default_dot() -> String {
    ".".to_string()
}

#[cfg(feature = "outline")]
#[derive(Debug, Clone, Deserialize)]
pub(super) struct OutlineArgs {
    pub(super) path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SlocArgs {
    #[serde(default = "default_dot", alias = "root")]
    pub(super) path: String,
    #[serde(default)]
    pub(super) exclude: Option<ExcludeArg>,
}
