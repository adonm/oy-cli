use diffy::create_patch;

use super::output::ChangedFileOutput;

pub(super) fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let diff = create_patch(old, new).to_string();
    let diff = diff
        .strip_prefix("--- original\n+++ modified\n")
        .map(|body| format!("--- {path}\n+++ {path}\n{body}"))
        .unwrap_or(diff);
    crate::ui::head_tail(&diff, 12000).0
}

pub(super) fn combined_diff(files: &[ChangedFileOutput]) -> String {
    let text = files
        .iter()
        .map(|item| item.diff.as_str())
        .filter(|diff| !diff.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    crate::ui::head_tail(&text, 12000).0
}
