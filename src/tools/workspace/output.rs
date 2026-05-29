use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(super) struct ListOutput {
    pub path: String,
    pub items: Vec<String>,
    pub count: usize,
    pub truncated: bool,
    pub exclude: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReadOutput {
    pub path: String,
    pub offset: usize,
    pub limit: usize,
    pub text: String,
    pub line_count: usize,
    pub truncated: bool,
    pub checksum: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct ReadMultipleFilesOutput {
    pub files: Vec<ReadOutput>,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchHit {
    pub path: String,
    pub line_number: usize,
    pub column: usize,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ToolErrorItem {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SearchOutput {
    pub pattern: String,
    pub mode: &'static str,
    pub warning: Option<String>,
    pub read_path: Option<String>,
    pub file_count: usize,
    pub path: String,
    pub match_count: usize,
    pub matches: Vec<SearchHit>,
    pub truncated: bool,
    pub exclude: Option<Vec<String>>,
    pub errors: Option<Vec<ToolErrorItem>>,
}

#[derive(Debug, Serialize)]
pub(super) struct ChangedFileOutput {
    pub path: String,
    pub replacements: usize,
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SkippedFileOutput {
    pub path: String,
    pub reason: &'static str,
}

#[derive(Debug, Serialize)]
pub(super) struct ReplaceOutput {
    pub pattern: String,
    pub replacement: String,
    pub mode: &'static str,
    pub path: String,
    pub changed_file_count: usize,
    pub replacement_count: usize,
    pub changed_files: Vec<ChangedFileOutput>,
    pub diff: String,
    pub truncated: bool,
    pub exclude: Option<Vec<String>>,
    pub skipped: Vec<SkippedFileOutput>,
    pub errors: Vec<ToolErrorItem>,
}

#[derive(Debug, Serialize)]
pub(super) struct PatchChangedFileOutput {
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub(super) struct PatchOutput {
    pub patch_count: usize,
    pub changed_file_count: usize,
    pub changed_files: Vec<PatchChangedFileOutput>,
    pub diff: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct SlocOutput {
    pub path: String,
    pub format: &'static str,
    pub output: Value,
    pub exclude: Option<Vec<String>>,
}
