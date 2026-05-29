use super::*;

#[test]
fn non_interactive_default_denies_patch() {
    let (dir, ctx) = test_context(ToolPolicy::with_write(Approval::Ask, Approval::Ask), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let err = workspace::tool_patch(
        &ctx,
        PatchArgs {
            patch: "--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\n".into(),
            strip: 1,
            limit: 10,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("requires interactive approval"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
}

#[tokio::test]
async fn invoke_shared_records_external_side_effect_attempts() {
    let (dir, ctx) = test_context(ToolPolicy::read_only(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let shared = std::sync::Arc::new(tokio::sync::Mutex::new(ctx));

    let err = invoke_shared(
        shared.clone(),
        "replace",
        json!({"pattern": "one", "replacement": "two", "path": "a.txt", "mode": "literal"}),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("tool denied by policy: replace"));
    assert!(shared.lock().await.external_side_effects());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
}

#[tokio::test]
async fn invoke_shared_read_only_tools_do_not_mark_external_side_effects() {
    let (dir, ctx) = test_context(ToolPolicy::read_only(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let shared = std::sync::Arc::new(tokio::sync::Mutex::new(ctx));

    let value = invoke_shared(shared.clone(), "read", json!({"path": "a.txt"}))
        .await
        .unwrap();

    assert_eq!(value["path"], "a.txt");
    assert!(!shared.lock().await.external_side_effects());
}

#[test]
fn non_interactive_default_denies_replace() {
    let (dir, ctx) = test_context(ToolPolicy::with_write(Approval::Ask, Approval::Ask), false);
    fs::write(dir.path().join("a.txt"), "one").unwrap();
    let err = workspace::tool_replace(
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
    .unwrap_err();
    assert!(err.to_string().contains("requires interactive approval"));
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one");
}

#[test]
fn file_tools_deny_out_of_workspace_paths_in_all_modes() {
    for policy in [
        ToolPolicy::with_write(Approval::Ask, Approval::Ask),
        auto_policy(),
        ToolPolicy::read_only(),
    ] {
        let (_dir, ctx) = test_context(policy, false);
        let err = workspace::tool_read(
            &ctx,
            ReadArgs {
                path: "/etc/hosts".into(),
                offset: 1,
                limit: 1,
                tail_lines: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("path outside workspace"));
    }
}

#[test]
fn read_only_exposes_research_tools_but_not_mutation_tools() {
    let (_dir, ctx) = test_context(ToolPolicy::read_only(), false);
    let names = tool_specs(&ctx)
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    for expected in ["list", "read", "search", "sloc", "webfetch", "todo"] {
        assert!(
            names.iter().any(|name| name.as_str() == expected),
            "missing {expected}"
        );
    }
    for denied in ["replace", "patch", "bash"] {
        assert!(
            !names.iter().any(|name| name.as_str() == denied),
            "exposed {denied}"
        );
    }
}

#[tokio::test]
async fn invoke_accepts_numeric_strings_and_aliases() {
    let (dir, mut ctx) = test_context(auto_policy(), false);
    fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();

    let value = invoke(
        &mut ctx,
        "read",
        json!({"file": "a.txt", "start": "2", "lines": "1"}),
    )
    .await
    .unwrap();

    assert_eq!(value["offset"], 2);
    assert_eq!(value["limit"], 1);
    assert_eq!(value["text"], "two");
}
