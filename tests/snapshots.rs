#![allow(dead_code)]

use serde_json::json;
use std::sync::Mutex;

static OUTPUT_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[path = "../src/agent.rs"]
mod agent;
#[path = "../src/chat.rs"]
mod chat;
#[path = "../src/config.rs"]
mod config;
#[path = "../src/model.rs"]
mod model;
#[path = "../src/tools/mod.rs"]
mod tools;
#[path = "../src/ui.rs"]
mod ui;

#[test]
fn chat_help_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    insta::assert_snapshot!(chat::chat_help_text());
}

#[test]
fn tool_preview_normal_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ui::set_output_mode(ui::OutputMode::Normal);
    let value = json!({
        "pattern": "run_prompt",
        "path": "src",
        "match_count": 2,
        "matches": [
            {"path": "src/agent.rs", "line_number": 283, "column": 1, "text": "pub async fn run_prompt(...)"},
            {"path": "src/ui.rs", "line_number": 410, "column": 22, "text": "agent::run_prompt(...)"}
        ],
        "truncated": false
    });
    insta::assert_snapshot!(tools::preview_tool_output("search", &value));
    ui::set_output_mode(ui::OutputMode::Normal);
}

#[test]
fn tool_preview_verbose_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ui::set_output_mode(ui::OutputMode::Verbose);
    let value = json!({
        "path": "src/main.rs",
        "offset": 1,
        "limit": 3,
        "text": "fn main() {\n    println!(\"hi\");\n}",
        "line_count": 10,
        "truncated": true
    });
    insta::assert_snapshot!(tools::preview_tool_output("read", &value));
    ui::set_output_mode(ui::OutputMode::Normal);
}

#[test]
fn tool_preview_bash_failure_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ui::set_output_mode(ui::OutputMode::Normal);
    let value = json!({
        "returncode": 2,
        "stdout": "",
        "stderr": "missing file
usage: demo <path>
",
        "stdout_truncated": false,
        "stderr_truncated": false
    });
    insta::assert_snapshot!(tools::preview_tool_output("bash", &value));
    ui::set_output_mode(ui::OutputMode::Normal);
}

#[test]
fn tool_preview_replace_normal_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ui::set_output_mode(ui::OutputMode::Normal);
    let value = json!({
        "changed_file_count": 2,
        "replacement_count": 3,
        "changed_files": [
            {"path": "src/lib.rs", "replacements": 1},
            {"path": "src/main.rs", "replacements": 2}
        ],
        "truncated": false
    });
    insta::assert_snapshot!(tools::preview_tool_output("replace", &value));
    ui::set_output_mode(ui::OutputMode::Normal);
}

#[test]
fn tool_preview_webfetch_snapshot() {
    let _guard = OUTPUT_MODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    ui::set_output_mode(ui::OutputMode::Normal);
    let value = json!({
        "status_code": 200,
        "url": "https://example.com/docs",
        "text": "# docs
hello
",
        "format": "markdown",
        "truncated": false
    });
    insta::assert_snapshot!(tools::preview_tool_output("webfetch", &value));
    ui::set_output_mode(ui::OutputMode::Normal);
}
