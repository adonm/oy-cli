use super::*;

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
