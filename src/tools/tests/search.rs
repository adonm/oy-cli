use super::*;
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
fn search_exact_file_does_not_spend_limit_on_siblings() {
    let (dir, ctx) = test_context(auto_policy(), false);
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/aaa.rs"), "hit\n".repeat(10_050)).unwrap();
    fs::write(dir.path().join("src/target.rs"), "hit\nhit\n").unwrap();

    let value = workspace::tool_search(
        &ctx,
        SearchArgs {
            pattern: "hit".into(),
            path: "src/target.rs".into(),
            exclude: None,
            limit: 2,
            mode: SearchMode::Literal,
        },
    )
    .unwrap();

    assert_eq!(value["match_count"], 2);
    assert_eq!(value["matches"][0]["path"], "src/target.rs");
    assert_eq!(value["matches"][1]["path"], "src/target.rs");
    assert_eq!(value["truncated"], false);
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
