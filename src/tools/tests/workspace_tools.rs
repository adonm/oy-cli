use super::*;

#[test]
fn auto_policy_allows_patch() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    fs::write(dir.path().join("b.txt"), "alpha\n").unwrap();
    let value = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\ndiff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-alpha\n+beta\n".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(value["patch_count"], 2);
    assert_eq!(value["changed_file_count"], 2);
    assert!(value["diff"].as_str().unwrap().contains("--- a.txt"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "beta\n"
    );
}

#[test]
fn patch_accepts_apply_patch_update_file_format() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(
        dir.path().join("engine.rs"),
        "impl BrowserEngine {\n    /// The full browser request.\n    pub fn request(&self) {}\n\n    /// Register a callback.\n    pub fn response(&self) {}\n}\n",
    )
    .unwrap();

    let value = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "*** Begin Patch\n*** Update File: engine.rs\n@@\n-    /// The full browser request.\n+    /// The full browser URI request.\n     pub fn request(&self) {}\n@@ Register a callback\n-    /// Register a callback.\n+    /// Register a response callback.\n     pub fn response(&self) {}\n*** End Patch\n"
                .into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap();

    assert_eq!(value["patch_count"], 1);
    assert_eq!(value["changed_file_count"], 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("engine.rs")).unwrap(),
        "impl BrowserEngine {\n    /// The full browser URI request.\n    pub fn request(&self) {}\n\n    /// Register a response callback.\n    pub fn response(&self) {}\n}\n"
    );
}

#[test]
fn apply_patch_rejects_add_file_and_leaves_workspace_unchanged() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();

    let err = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "*** Begin Patch\n*** Add File: new.txt\n+new\n*** End Patch\n".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("file creation patches are not supported")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
    assert!(!dir.path().join("new.txt").exists());
}

#[test]
fn patch_without_trailing_newline() {
    // diffy parses incorrectly when the patch lacks a trailing newline.
    // This test covers both: last line is an insert (silent corruption),
    // and last line is context (apply error).
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("f.txt"), "apple\nbanana\ncherry\ndate\n").unwrap();

    // Case 1: last hunk line is an insert, no trailing newline
    let _value = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "--- a/f.txt\n+++ b/f.txt\n@@ -3 +3 @@\n-cherry\n+CHERRY".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        "apple\nbanana\nCHERRY\ndate\n",
        "insert without trailing newline should not corrupt subsequent lines"
    );

    // Case 2: last hunk line is a context line, no trailing newline
    fs::write(dir.path().join("g.txt"), "alpha\nbeta\ngamma\ndelta\n").unwrap();
    let _value = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "--- a/g.txt\n+++ b/g.txt\n@@ -2,3 +2,3 @@\n beta\n-gamma\n+GAMMA\n delta"
                .into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("g.txt")).unwrap(),
        "alpha\nbeta\nGAMMA\ndelta\n",
        "context without trailing newline should apply cleanly"
    );
}

#[test]
fn patch_default_strip_falls_back_to_raw_paths() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("raw.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let value = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "--- raw.txt\n+++ raw.txt\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n"
                .into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap();

    assert_eq!(value["changed_file_count"], 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("raw.txt")).unwrap(),
        "alpha\nBETA\ngamma\n"
    );
}

#[test]
fn patch_apply_error_mentions_hunk_and_reread_hint() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("stale.txt"), "left\nactual\nright\n").unwrap();

    let err = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "--- a/stale.txt\n+++ b/stale.txt\n@@ -1,3 +1,3 @@\n left\n-expected\n+EXPECTED\n right\n".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap_err();
    let err = err.to_string();
    assert!(err.contains("failed applying patch for stale.txt"));
    assert!(err.contains("error applying hunk #1"));
    assert!(err.contains("re-read the file"));
}

#[test]
fn patch_rejects_create_and_leaves_workspace_unchanged() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let err = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new\n".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("file creation patches are not supported")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
    assert!(!dir.path().join("new.txt").exists());
}

#[test]
fn patch_rejects_parent_directory_paths() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let err = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "--- a/../a.txt\n+++ b/../a.txt\n@@ -1 +1 @@\n-one\n+two\n".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("path outside workspace"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
}

#[test]
fn auto_policy_allows_replace() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "one").unwrap();
    let value = workspace::tool_replace(
        &ctx,
        ReplaceArgs {
            pattern: "one".into(),
            replacement: "two".into(),
            path: "a.txt".into(),
            exclude: None,
            limit: 10,
            mode: ReplaceMode::Regex,
        },
    )
    .unwrap();
    assert_eq!(value["replacement_count"], 1);
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "two");
}

#[test]
fn sloc_accepts_space_separated_paths() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/app.rs"), "fn app() {}\n").unwrap();
    fs::write(dir.path().join("README.md"), "# docs\n").unwrap();
    fs::write(dir.path().join("ignored.rs"), "fn ignored() {}\n").unwrap();

    let value = workspace::tool_sloc(
        &ctx,
        SlocArgs {
            path: "src README.md".into(),
            exclude: None,
        },
    )
    .unwrap();

    assert_eq!(value["path"], "src README.md");
    assert_eq!(value["output"]["Rust"]["code"], 1);
    assert_eq!(value["output"]["Markdown"]["comments"], 1);
    assert!(value["output"]["Total"].is_object());
}

#[test]
fn search_accepts_space_separated_paths() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/app.rs"), "fn app_hit() {}\n").unwrap();
    fs::write(dir.path().join("src/ui.rs"), "fn ui_hit() {}\n").unwrap();
    fs::write(dir.path().join("src/other.rs"), "fn other_hit() {}\n").unwrap();

    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "fn (app|ui)_hit".into(),
            path: "src/app.rs src/ui.rs".into(),
            exclude: None,
            limit: 10,
            mode: SearchMode::Regex,
        },
    )
    .unwrap();

    assert_eq!(value["match_count"], 2);
    let paths = value["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(paths.iter().any(|path| path == "src/app.rs"));
    assert!(paths.iter().any(|path| path == "src/ui.rs"));
    assert!(!paths.iter().any(|path| path == "src/other.rs"));
}

#[test]
fn search_auto_falls_back_to_literal_for_invalid_regex() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("notes.txt"), "literal [text\n").unwrap();

    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "[text".into(),
            path: "notes.txt".into(),
            exclude: None,
            limit: 10,
            mode: SearchMode::Auto,
        },
    )
    .unwrap();

    assert_eq!(value["mode"], "literal");
    assert_eq!(value["match_count"], 1);
    assert!(
        value["warning"]
            .as_str()
            .unwrap()
            .contains("searched literally")
    );
}

#[test]
fn search_auto_treats_plain_identifier_as_literal_and_suggests_read_path() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "foo.bar\nfooXbar\n").unwrap();
    fs::write(dir.path().join("b.txt"), "foo.bar\n").unwrap();

    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "foo.bar".into(),
            path: ".".into(),
            exclude: None,
            limit: 10,
            mode: SearchMode::Auto,
        },
    )
    .unwrap();

    assert_eq!(value["mode"], "regex");
    assert_eq!(value["match_count"], 3);
    assert_eq!(value["read_path"], "a.txt");
    assert_eq!(value["file_count"], 2);

    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "foo_bar".into(),
            path: ".".into(),
            exclude: None,
            limit: 10,
            mode: SearchMode::Auto,
        },
    )
    .unwrap();

    assert_eq!(value["mode"], "literal");
    assert_eq!(value["match_count"], 0);
}

#[test]
fn list_supports_fuzzy_file_queries() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::create_dir_all(dir.path().join("src/tools")).unwrap();
    fs::write(dir.path().join("src/tools/workspace.rs"), "").unwrap();
    fs::write(dir.path().join("README.md"), "").unwrap();

    let value = workspace::tool_list(
        &ctx,
        ListArgs {
            path: "wrkspc".into(),
            exclude: None,
            limit: 10,
        },
    )
    .unwrap();

    let items = value["items"].as_array().unwrap();
    assert_eq!(items.first().unwrap(), "src/tools/workspace.rs");
}

#[test]
fn read_missing_path_suggests_fuzzy_matches_without_reading_them() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::create_dir_all(dir.path().join("src/tools")).unwrap();
    fs::write(
        dir.path().join("src/tools/workspace.rs"),
        "SECRET_SENTINEL\n",
    )
    .unwrap();
    fs::write(dir.path().join("README.md"), "readme\n").unwrap();

    let err = workspace::tool_read(
        &ctx,
        ReadArgs {
            path: "wrkspc".into(),
            offset: 1,
            limit: 10,
        },
    )
    .unwrap_err();
    let message = err.to_string();

    assert!(message.contains("path does not exist: wrkspc"));
    assert!(message.contains("did you mean src/tools/workspace.rs"));
    assert!(message.contains("exact existing workspace file path"));
    assert!(!message.contains("SECRET_SENTINEL"));
}

#[test]
fn directory_exclude_applies_to_search_and_replace_file_targets() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::create_dir(dir.path().join("generated")).unwrap();
    fs::write(dir.path().join("generated/a.txt"), "hit\n").unwrap();
    fs::write(dir.path().join("keep.txt"), "hit\n").unwrap();

    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "hit".into(),
            path: ".".into(),
            exclude: Some(ExcludeArg::String("generated/".into())),
            limit: 10,
            mode: SearchMode::Literal,
        },
    )
    .unwrap();
    assert_eq!(value["match_count"], 1);
    assert_eq!(value["matches"][0]["path"], "keep.txt");

    let value = workspace::tool_replace(
        &ctx,
        ReplaceArgs {
            pattern: "hit".into(),
            replacement: "miss".into(),
            path: "generated/a.txt".into(),
            exclude: Some(ExcludeArg::String("generated/".into())),
            limit: 10,
            mode: ReplaceMode::Literal,
        },
    )
    .unwrap();
    assert_eq!(value["replacement_count"], 0);
    assert_eq!(
        fs::read_to_string(dir.path().join("generated/a.txt")).unwrap(),
        "hit\n"
    );
}

#[test]
fn replace_literal_treats_pattern_and_dollars_as_plain_text() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "a+b $1\n").unwrap();

    let value = workspace::tool_replace(
        &ctx,
        ReplaceArgs {
            pattern: "a+b".into(),
            replacement: "$1".into(),
            path: "a.txt".into(),
            exclude: None,
            limit: 10,
            mode: ReplaceMode::Literal,
        },
    )
    .unwrap();

    assert_eq!(value["mode"], "literal");
    assert_eq!(value["replacement_count"], 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "$1 $1\n"
    );
}

#[test]
fn read_rejects_oversized_workspace_file_before_loading() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(
        dir.path().join("large.txt"),
        vec![b'x'; workspace::MAX_WORKSPACE_FILE_BYTES as usize + 1],
    )
    .unwrap();
    let err = workspace::tool_read(
        &ctx,
        ReadArgs {
            path: "large.txt".into(),
            offset: 1,
            limit: 1,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("workspace read cap"));
}

#[test]
fn search_stops_at_requested_limit() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("hits.txt"), "hit\nhit\nhit\n").unwrap();
    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "hit".into(),
            path: "hits.txt".into(),
            exclude: None,
            limit: 2,
            mode: SearchMode::Literal,
        },
    )
    .unwrap();
    assert_eq!(value["match_count"], 2);
    assert_eq!(value["matches"].as_array().unwrap().len(), 2);
    assert_eq!(value["truncated"], true);
}

#[test]
fn replace_skips_oversized_workspace_file() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(
        dir.path().join("large.txt"),
        vec![b'x'; workspace::MAX_WORKSPACE_FILE_BYTES as usize + 1],
    )
    .unwrap();
    let value = workspace::tool_replace(
        &ctx,
        ReplaceArgs {
            pattern: "x".into(),
            replacement: "y".into(),
            path: "large.txt".into(),
            exclude: None,
            limit: 10,
            mode: ReplaceMode::Literal,
        },
    )
    .unwrap();
    assert_eq!(value["replacement_count"], 0);
    assert_eq!(
        value["skipped"][0]["reason"],
        "file exceeds workspace read cap"
    );
}

#[test]
fn search_file_treats_zip_as_binary_file() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("sample.zip"), b"PK\0\0not searched").unwrap();
    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "not searched".into(),
            path: "sample.zip".into(),
            exclude: None,
            limit: 10,
            mode: SearchMode::Literal,
        },
    )
    .unwrap();
    assert_eq!(value["match_count"], 0);
}

#[test]
fn read_rejects_zip_virtual_member() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("sample.zip"), b"PK\0\0").unwrap();
    let err = workspace::tool_read(
        &ctx,
        ReadArgs {
            path: "sample.zip::docs/readme.txt".into(),
            offset: 1,
            limit: 10,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("path does not exist"));
}

#[test]
fn list_does_not_expand_zip_members() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("sample.zip"), b"PK\0\0").unwrap();
    let value = workspace::tool_list(
        &ctx,
        ListArgs {
            path: "sample.zip".into(),
            exclude: None,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(value["count"], 1);
    let items = value["items"].as_array().unwrap();
    assert_eq!(items, &vec![json!("sample.zip")]);
}

#[cfg(unix)]
#[test]
fn list_does_not_follow_symlink_globs_outside_workspace() {
    use std::os::unix::fs::symlink;

    let (dir, ctx) = test_context(auto_policy(), false);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret-name.txt"), "secret").unwrap();
    symlink(outside.path(), dir.path().join("link")).unwrap();

    let value = workspace::tool_list(
        &ctx,
        ListArgs {
            path: "link/*".into(),
            exclude: None,
            limit: 10,
        },
    )
    .unwrap();

    assert_eq!(value["count"], 0);
    assert!(value["items"].as_array().unwrap().is_empty());
}
