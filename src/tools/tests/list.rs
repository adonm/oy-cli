use super::*;
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
