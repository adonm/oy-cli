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

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SighthoundAnalysis {
    #[default]
    All,
    Simple,
    Taint,
}

impl SighthoundAnalysis {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Simple => "simple",
            Self::Taint => "taint",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SighthoundArgs {
    #[serde(default = "default_dot", alias = "root")]
    pub(super) path: String,
    #[serde(default)]
    pub(super) analysis: SighthoundAnalysis,
    #[serde(default)]
    pub(super) language: Option<String>,
    #[serde(default)]
    pub(super) include_test_fixtures: bool,
    #[serde(default = "default_sighthound_findings")]
    pub(super) max_findings: usize,
}

fn default_sighthound_findings() -> usize {
    100
}
