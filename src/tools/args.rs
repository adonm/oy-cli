use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

use super::{DEFAULT_LIMIT, DEFAULT_WEBFETCH_TIMEOUT_SECONDS, TodoStatus};

#[derive(Debug, Clone, Deserialize)]
pub(super) struct TodoItemInput {
    #[serde(default)]
    pub(super) id: Option<String>,
    pub(super) task: String,
    #[serde(default)]
    pub(super) status: TodoStatus,
}

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
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SearchMode {
    Auto,
    Regex,
    Literal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReplaceMode {
    Regex,
    Literal,
}

fn default_search_mode() -> SearchMode {
    SearchMode::Auto
}

fn default_replace_mode() -> ReplaceMode {
    ReplaceMode::Regex
}

fn deserialize_usize<'de, D>(deserializer: D) -> std::result::Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Integer(usize),
        String(String),
    }
    match Number::deserialize(deserializer)? {
        Number::Integer(value) => Ok(value),
        Number::String(value) => value.trim().parse::<usize>().map_err(|_| {
            serde::de::Error::custom(format!("expected unsigned integer, got {value:?}"))
        }),
    }
}

fn deserialize_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Integer(u64),
        String(String),
    }
    match Number::deserialize(deserializer)? {
        Number::Integer(value) => Ok(value),
        Number::String(value) => value.trim().parse::<u64>().map_err(|_| {
            serde::de::Error::custom(format!("expected unsigned integer, got {value:?}"))
        }),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ListArgs {
    #[serde(default = "default_glob", alias = "root")]
    pub(super) path: String,
    #[serde(default)]
    pub(super) exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_usize")]
    pub(super) limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ReadArgs {
    #[serde(alias = "file")]
    pub(super) path: String,
    #[serde(
        default = "default_offset",
        alias = "start",
        deserialize_with = "deserialize_usize"
    )]
    pub(super) offset: usize,
    #[serde(
        default = "default_limit",
        alias = "lines",
        deserialize_with = "deserialize_usize"
    )]
    pub(super) limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SearchArgs {
    #[serde(alias = "query", alias = "regex")]
    pub(super) pattern: String,
    #[serde(default = "default_dot", alias = "root")]
    pub(super) path: String,
    #[serde(default)]
    pub(super) exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_usize")]
    pub(super) limit: usize,
    #[serde(default = "default_search_mode")]
    pub(super) mode: SearchMode,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ReplaceArgs {
    pub(super) pattern: String,
    pub(super) replacement: String,
    #[serde(default = "default_dot", alias = "root")]
    pub(super) path: String,
    #[serde(default)]
    pub(super) exclude: Option<ExcludeArg>,
    #[serde(default = "default_limit", deserialize_with = "deserialize_usize")]
    pub(super) limit: usize,
    #[serde(default = "default_replace_mode")]
    pub(super) mode: ReplaceMode,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SlocArgs {
    #[serde(default = "default_dot", alias = "root")]
    pub(super) path: String,
    #[serde(default)]
    pub(super) exclude: Option<ExcludeArg>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct BashArgs {
    #[serde(alias = "cmd")]
    pub(super) command: String,
    #[serde(default = "default_bash_timeout", deserialize_with = "deserialize_u64")]
    pub(super) timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct WebfetchArgs {
    pub(super) url: String,
    #[serde(default = "default_method")]
    pub(super) method: String,
    #[serde(default)]
    pub(super) headers: HeaderPolicy,
    #[serde(default)]
    pub(super) redirects: RedirectPolicy,
    #[serde(default = "default_web_timeout", deserialize_with = "deserialize_u64")]
    pub(super) timeout_seconds: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(from = "Option<BTreeMap<String, String>>")]
pub(super) struct HeaderPolicy {
    pub(super) values: BTreeMap<String, String>,
}

impl From<Option<BTreeMap<String, String>>> for HeaderPolicy {
    fn from(values: Option<BTreeMap<String, String>>) -> Self {
        Self {
            values: values.unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "bool")]
pub(super) enum RedirectPolicy {
    None,
    #[default]
    Follow,
}

impl From<bool> for RedirectPolicy {
    fn from(follow: bool) -> Self {
        match follow {
            true => Self::Follow,
            false => Self::None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct AskArgs {
    pub(super) question: String,
    #[serde(default)]
    pub(super) choices: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct TodoArgs {
    #[serde(default, alias = "items")]
    pub(super) todos: Vec<TodoItemInput>,
    #[serde(default)]
    pub(super) persist: bool,
}

fn default_glob() -> String {
    "*".to_string()
}
fn default_dot() -> String {
    ".".to_string()
}
fn default_limit() -> usize {
    DEFAULT_LIMIT
}
fn default_offset() -> usize {
    1
}
fn default_bash_timeout() -> u64 {
    120
}
fn default_method() -> String {
    "GET".to_string()
}
fn default_web_timeout() -> u64 {
    DEFAULT_WEBFETCH_TIMEOUT_SECONDS
}
