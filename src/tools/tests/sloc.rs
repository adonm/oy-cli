use super::*;
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

