//! Human-readable tool call and tool result previews.
//!
//! The registry calls these helpers for approval prompts and progress output;
//! keep them concise, bounded, and free of side effects.
//!
//! Each tool category lives in its own sub-module under `preview/`:
//! - `common`   — shared helpers, value extraction, and formatting
//! - `workspace` — list, read, search, replace, patch, sloc, outline
//! - `network`   — webfetch, repo_clone
//! - `process`   — bash
//! - `planning`  — todo, think, ask

mod common;
mod network;
mod planning;
mod process;
mod workspace;

#[allow(unused_imports)]
pub(super) use common::{
    append_capped_flag, append_preview_lines, append_search_hit_block, append_search_hits,
    bool_marker, compact_kvs, count_files_in_matches, count_lines, format_search_hit_line,
    limited_preview_body, output_preview, plural, preview_generic, preview_value,
    todo_call_summary, tool_call_summary, tool_output, truncation_flag, value_bool, value_str,
    value_usize, verbose_preview, with_verbose,
};
pub(super) use network::{
    preview_repo_clone, preview_webfetch, summary_repo_clone, summary_webfetch,
};
pub(super) use planning::{
    preview_ask, preview_think, preview_todo, summary_ask, summary_think, summary_todo,
};
pub(super) use process::{preview_bash, summary_bash};
pub(super) use workspace::{
    preview_list, preview_outline, preview_patch, preview_read, preview_read_multiple_files,
    preview_replace, preview_search, preview_sloc, summary_list, summary_outline, summary_patch,
    summary_read, summary_read_multiple_files, summary_replace, summary_search, summary_sloc,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    static OUTPUT_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn search_preview_groups_repeated_file_hits_without_expanding_unique_file_hits() {
        let repeated = json!({
            "pattern": "fn",
            "path": "src/main.rs",
            "match_count": 2,
            "matches": [
                {"path": "src/main.rs", "line_number": 1, "column": 1, "text": "fn main()"},
                {"path": "src/main.rs", "line_number": 2, "column": 5, "text": "fn helper()"}
            ],
            "truncated": false
        });
        let unique = json!({
            "pattern": "fn",
            "path": "src",
            "match_count": 2,
            "matches": [
                {"path": "src/main.rs", "line_number": 1, "column": 1, "text": "fn main()"},
                {"path": "src/lib.rs", "line_number": 2, "column": 5, "text": "fn helper()"}
            ],
            "truncated": false
        });

        let repeated_output = strip_ansi_escapes::strip_str(tool_output("search", &repeated));
        let unique_output = strip_ansi_escapes::strip_str(tool_output("search", &unique));
        assert_eq!(repeated_output.matches("── src/main.rs").count(), 1);
        assert!(repeated_output.contains("fn main()"));
        assert!(repeated_output.contains("fn helper()"));
        assert_eq!(unique_output.matches("── src/main.rs").count(), 1);
        assert_eq!(unique_output.matches("── src/lib.rs").count(), 1);
    }

    #[test]
    fn bash_preview_uses_bounded_preview_fields_and_marks_capped_output() {
        let value = json!({
            "returncode": 0,
            "stdout": "full output should not be shown",
            "stdout_preview": "preview head\npreview tail",
            "stderr": "",
            "stdout_truncated": true,
            "stderr_truncated": false,
            "stdout_capped": true,
            "stderr_capped": false
        });

        let output = tool_output("bash", &value);

        assert!(output.contains("stdout-capped=yes"));
        assert!(output.contains("preview head"));
        assert!(!output.contains("full output should not be shown"));
    }

    #[test]
    fn tool_preview_replace_normal_snapshot() {
        let _guard = OUTPUT_MODE_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
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
        insta::assert_snapshot!(tool_output("replace", &value));
        crate::ui::set_output_mode(crate::ui::OutputMode::Normal);
    }
}
