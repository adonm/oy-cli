use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(super) struct OutlineOutput {
    pub path: String,
    pub format: &'static str,
    pub command: String,
    pub output: Value,
}

#[derive(Debug, Serialize)]
pub(super) struct SlocOutput {
    pub path: String,
    pub format: &'static str,
    pub output: Value,
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub(super) struct SighthoundOutput {
    pub path: String,
    pub format: &'static str,
    pub command: String,
    pub analysis: &'static str,
    pub effective_analysis: &'static str,
    pub status: &'static str,
    pub language: Option<String>,
    pub finding_count: usize,
    pub returned_count: usize,
    pub truncated: bool,
    pub findings: Value,
}
