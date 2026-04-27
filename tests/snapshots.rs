use oy::{OutputMode, chat_help_text, preview_tool_output, set_output_mode};
use serde_json::json;
use std::sync::Mutex;

static OUTPUT_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn chat_help_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    insta::assert_snapshot!(chat_help_text());
}

#[test]
fn tool_preview_normal_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    set_output_mode(OutputMode::Normal);
    let value = json!({
        "pattern": "run_prompt",
        "path": "src",
        "match_count": 6,
        "matches": [
            {"path": "src/session.rs", "line_number": 283, "column": 1, "text": "pub async fn run_prompt(...)"},
            {"path": "src/app.rs", "line_number": 40, "column": 9, "text": "Run(RunArgs),"},
            {"path": "src/chat.rs", "line_number": 110, "column": 18, "text": "run_prompt from chat"},
            {"path": "src/model.rs", "line_number": 88, "column": 5, "text": "resolve model before run_prompt"},
            {"path": "src/tools.rs", "line_number": 500, "column": 13, "text": "tool output for run_prompt"},
            {"path": "src/ui.rs", "line_number": 410, "column": 22, "text": "session::run_prompt(...)"}
        ],
        "truncated": false
    });
    insta::assert_snapshot!(preview_tool_output("search", &value));
    set_output_mode(OutputMode::Normal);
}

#[test]
fn tool_preview_verbose_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    set_output_mode(OutputMode::Verbose);
    let value = json!({
        "path": "src/main.rs",
        "offset": 1,
        "limit": 3,
        "text": "fn main() {\n    println!(\"hi\");\n}",
        "line_count": 10,
        "truncated": true
    });
    insta::assert_snapshot!(preview_tool_output("read", &value));
    set_output_mode(OutputMode::Normal);
}

#[test]
fn tool_preview_bash_failure_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    set_output_mode(OutputMode::Normal);
    let value = json!({
        "returncode": 2,
        "stdout": "",
        "stderr": "missing file\nusage: demo <path>\ntry --help\nexample: demo Cargo.toml\nerror code E2\nignored tail\n",
        "stdout_truncated": false,
        "stderr_truncated": false
    });
    insta::assert_snapshot!(preview_tool_output("bash", &value));
    set_output_mode(OutputMode::Normal);
}

#[test]
fn tool_preview_replace_normal_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    set_output_mode(OutputMode::Normal);
    let value = json!({
        "changed_file_count": 6,
        "replacement_count": 9,
        "changed_files": [
            {"path": "src/lib.rs", "replacements": 1},
            {"path": "src/main.rs", "replacements": 2},
            {"path": "src/app.rs", "replacements": 1},
            {"path": "src/config.rs", "replacements": 2},
            {"path": "src/tools.rs", "replacements": 1},
            {"path": "README.md", "replacements": 2}
        ],
        "truncated": false
    });
    insta::assert_snapshot!(preview_tool_output("replace", &value));
    set_output_mode(OutputMode::Normal);
}

#[test]
fn tool_preview_webfetch_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    set_output_mode(OutputMode::Normal);
    let value = json!({
        "status_code": 200,
        "url": "https://example.com/docs",
        "text": "# docs\nhello\ninstall\nconfigure\nrun\nextra\n",
        "format": "markdown",
        "truncated": false
    });
    insta::assert_snapshot!(preview_tool_output("webfetch", &value));
    set_output_mode(OutputMode::Normal);
}
