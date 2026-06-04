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
fn patch_supports_context_pure_insertion_hunks() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::write(
        dir.path().join("engine.rs"),
        "impl BrowserEngine {\n    /// The full browser request.\n    pub fn request(&self) {}\n}\n",
    )
    .unwrap();

    let value = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "*** Begin Patch\n*** Update File: engine.rs\n@@\n+    // Initialized\n@@ The full browser request.\n+    // Executed\n*** End Patch\n"
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
        "    // Initialized\nimpl BrowserEngine {\n    // Executed\n    /// The full browser request.\n    pub fn request(&self) {}\n}\n"
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
