use super::*;

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
            tail_lines: None,
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
            tail_lines: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("workspace read cap"));
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
            tail_lines: None,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("path does not exist"));
}
