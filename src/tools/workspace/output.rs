use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(super) struct SlocOutput {
    pub path: String,
    pub format: &'static str,
    pub output: Value,
    pub exclude: Option<Vec<String>>,
}
