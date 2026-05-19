use super::args::{
    BashArgs, ExcludeArg, HeaderPolicy, ListArgs, PatchArgs, ReadArgs, RedirectPolicy, ReplaceArgs,
    ReplaceMode, SearchArgs, SearchMode, SlocArgs, TodoArgs, TodoItemInput, WebfetchArgs,
};
use super::network::{is_public_ip, tool_webfetch, validated_webfetch_headers};
use super::todo::tool_todo;
use super::workspace;
use super::*;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;

fn test_context(policy: ToolPolicy, interactive: bool) -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        root: dir.path().to_path_buf(),
        interactive,
        policy,
        todos: Vec::new(),
        external_side_effects: false,
    };
    (dir, ctx)
}

fn auto_policy() -> ToolPolicy {
    ToolPolicy::with_write(Approval::Auto, Approval::Auto)
}

fn schema_for(name: &str) -> Value {
    let (_dir, ctx) = test_context(auto_policy(), true);
    tool_specs(&ctx)
        .into_iter()
        .find(|tool| tool.name.as_str() == name)
        .map(|tool| tool.parameters)
        .unwrap_or_else(|| panic!("missing schema for {name}"))
}

fn tool_description(name: &str) -> String {
    let (_dir, ctx) = test_context(auto_policy(), true);
    tool_specs(&ctx)
        .into_iter()
        .find(|tool| tool.name.as_str() == name)
        .map(|tool| tool.description)
        .unwrap_or_else(|| panic!("missing tool description for {name}"))
}

#[test]
fn tool_schemas_are_closed_objects_with_valid_required_fields() {
    let (_dir, ctx) = test_context(auto_policy(), true);
    for tool in tool_specs(&ctx) {
        let schema = tool.parameters;
        assert_eq!(schema["type"], "object", "{} type", tool.name);
        assert_eq!(
            schema["additionalProperties"], false,
            "{} additionalProperties",
            tool.name
        );
        let props = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("missing properties for {}", tool.name));
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required {
                let field = field.as_str().unwrap();
                assert!(
                    props.contains_key(field),
                    "{} requires unknown {field}",
                    tool.name
                );
            }
        }
    }
}

#[test]
fn tool_schema_helpers_preserve_aliases_defaults_and_nullable_shapes() {
    let todo = schema_for("todo");
    assert_eq!(todo["properties"]["persist"]["default"], false);
    assert_eq!(
        todo["properties"]["items"]["items"]["required"],
        json!(["task"])
    );
    assert_eq!(
        todo["properties"]["todos"]["description"],
        "Complete replacement todo list. Alias: items. Omit to return current list."
    );

    let list = schema_for("list");
    assert_eq!(list["properties"]["path"]["default"], "*");
    assert!(
        list["properties"]["path"]["description"]
            .as_str()
            .unwrap()
            .contains("fuzzy file query")
    );
    assert_eq!(
        list["properties"]["exclude"]["anyOf"][1]["items"],
        json!({"type": "string"})
    );

    let webfetch = schema_for("webfetch");
    assert_eq!(webfetch["required"], json!(["url"]));
    assert_eq!(webfetch["properties"]["follow_redirects"]["default"], true);
    assert_eq!(
        webfetch["properties"]["headers"]["type"],
        json!(["object", "null"])
    );
    assert!(
        webfetch["properties"]["headers"]["description"]
            .as_str()
            .unwrap()
            .contains("credential headers are rejected")
    );
}

#[test]
fn fff_backed_tooldefs_document_path_semantics() {
    assert!(tool_description("list").contains("fff-style file discovery"));
    assert!(tool_description("search").contains("fff grep over indexed files"));
    assert!(tool_description("replace").contains("fff-indexed workspace files"));

    let search = schema_for("search");
    assert!(
        search["properties"]["path"]["description"]
            .as_str()
            .unwrap()
            .contains("Globs and fuzzy paths are not accepted")
    );
    assert!(
        search["properties"]["exclude"]["description"]
            .as_str()
            .unwrap()
            .contains("fff-indexed search paths")
    );

    let replace = schema_for("replace");
    assert!(
        replace["properties"]["path"]["description"]
            .as_str()
            .unwrap()
            .contains("fff-indexed files")
    );
    assert!(
        replace["properties"]["limit"]["description"]
            .as_str()
            .unwrap()
            .contains("replacement still applies to all matched files")
    );
}

#[test]
fn webfetch_defaults_to_redirects_and_doc_friendly_headers() {
    let args: WebfetchArgs = serde_json::from_value(json!({
        "url": "https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-files-mounting-eks.md"
    }))
    .unwrap();
    assert!(args.redirects == RedirectPolicy::Follow);

    let headers = validated_webfetch_headers(&args.headers).unwrap();
    assert_eq!(headers.get(ACCEPT.as_str()).unwrap(), WEBFETCH_ACCEPT);
    assert!(
        headers
            .get(USER_AGENT.as_str())
            .unwrap()
            .starts_with("oy-cli/")
    );
}

#[test]
fn webfetch_custom_headers_override_defaults_but_sensitive_headers_stay_denied() {
    let custom = BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        ("X-Trace".to_string(), "1".to_string()),
    ]);
    let headers = validated_webfetch_headers(&HeaderPolicy { values: custom }).unwrap();
    assert_eq!(headers.get(ACCEPT.as_str()).unwrap(), "application/json");
    assert_eq!(headers.get("X-Trace").unwrap(), "1");

    let denied = BTreeMap::from([("Authorization".to_string(), "Bearer x".to_string())]);
    assert!(validated_webfetch_headers(&HeaderPolicy { values: denied }).is_err());

    let invalid = BTreeMap::from([("X-Trace".to_string(), "bad\r\nvalue".to_string())]);
    assert!(validated_webfetch_headers(&HeaderPolicy { values: invalid }).is_err());
}

#[test]
fn webfetch_ip_filter_rejects_non_public_ranges() {
    for ip in [
        "0.0.0.0",
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.0.1",
        "100.64.0.1",
        "198.18.0.1",
        "224.0.0.1",
        "240.0.0.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "fec0::1",
        "ff00::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
    ] {
        assert!(!is_public_ip(ip.parse().unwrap()), "{ip} should be denied");
    }
    for ip in ["1.1.1.1", "8.8.8.8", "192.0.0.9", "2606:4700:4700::1111"] {
        assert!(is_public_ip(ip.parse().unwrap()), "{ip} should be allowed");
    }
}

#[test]
fn schemas_document_lenient_numbers_and_match_modes() {
    let search = schema_for("search");
    assert_eq!(
        search["properties"]["limit"]["type"],
        json!(["integer", "string"])
    );
    assert_eq!(
        search["properties"]["mode"]["enum"],
        json!(["auto", "regex", "literal"])
    );

    let replace = schema_for("replace");
    assert_eq!(
        replace["properties"]["mode"]["enum"],
        json!(["regex", "literal"])
    );

    let patch = schema_for("patch");
    assert_eq!(patch["properties"]["strip"]["default"], json!(1));
    assert_eq!(
        patch["properties"]["limit"]["type"],
        json!(["integer", "string"])
    );

    let bash = schema_for("bash");
    assert_eq!(
        bash["properties"]["timeout_seconds"]["type"],
        json!(["integer", "string"])
    );
    assert!(
        bash["properties"]["command"]["description"]
            .as_str()
            .unwrap()
            .contains("Inspect first")
    );
}

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
    let shared = std::sync::Arc::new(std::sync::Mutex::new(ctx));

    let err = invoke_shared(
        shared.clone(),
        "replace",
        json!({"pattern": "one", "replacement": "two", "path": "a.txt", "mode": "literal"}),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("tool denied by policy: replace"));
    assert!(
        shared
            .lock()
            .expect("tool context mutex poisoned")
            .external_side_effects
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "one\n"
    );
}

#[tokio::test]
async fn invoke_shared_read_only_tools_do_not_mark_external_side_effects() {
    let (dir, ctx) = test_context(ToolPolicy::read_only(), false);
    fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(ctx));

    let value = invoke_shared(shared.clone(), "read", json!({"path": "a.txt"}))
        .await
        .unwrap();

    assert_eq!(value["path"], "a.txt");
    assert!(
        !shared
            .lock()
            .expect("tool context mutex poisoned")
            .external_side_effects
    );
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
fn read_only_allows_todo_memory_but_denies_persistence() {
    let (_dir, mut ctx) = test_context(ToolPolicy::read_only(), false);
    let value = tool_todo(
        &mut ctx,
        TodoArgs {
            todos: Some(vec![TodoItemInput {
                id: None,
                task: "plan work".into(),
                status: TodoStatus::Pending,
            }]),
            persist: false,
        },
    )
    .unwrap();
    assert_eq!(value["count"], 1);
    assert_eq!(ctx.todos[0].task, "plan work");

    let err = tool_todo(
        &mut ctx,
        TodoArgs {
            todos: None,
            persist: true,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("tool denied by policy"));
}

#[tokio::test]
async fn todo_omission_reads_and_explicit_empty_clears() {
    let (_dir, mut ctx) = test_context(
        ToolPolicy::with_write(Approval::Auto, Approval::Auto),
        false,
    );

    invoke(
        &mut ctx,
        "todo",
        json!({
            "todos": [{ "task": "first", "status": "pending" }]
        }),
    )
    .await
    .unwrap();
    assert_eq!(ctx.todos.len(), 1);

    let read = invoke(&mut ctx, "todo", json!({})).await.unwrap();
    assert_eq!(read["count"], 1);
    assert_eq!(ctx.todos.len(), 1);

    let cleared = invoke(&mut ctx, "todo", json!({ "todos": [] }))
        .await
        .unwrap();
    assert_eq!(cleared["count"], 0);
    assert!(ctx.todos.is_empty());
}

#[tokio::test]
async fn todo_accepts_items_alias_even_when_todos_is_also_present() {
    let (_dir, mut ctx) = test_context(
        ToolPolicy::with_write(Approval::Auto, Approval::Auto),
        false,
    );

    let result = invoke(
        &mut ctx,
        "todo",
        json!({
            "todos": [{ "task": "canonical", "status": "pending" }],
            "items": [{ "task": "alias", "status": "pending" }]
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["count"], 1);
    assert_eq!(ctx.todos[0].task, "canonical");
}

#[tokio::test]
async fn non_interactive_default_denies_bash() {
    let (_dir, ctx) = test_context(ToolPolicy::with_write(Approval::Ask, Approval::Ask), false);
    let err = tool_bash(
        &ctx,
        BashArgs {
            command: "echo nope".into(),
            timeout_seconds: 1,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("requires interactive approval"));
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
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("path outside workspace"));
    }
}

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
async fn webfetch_checks_network_policy_at_sink() {
    let (_dir, ctx) = test_context(
        ToolPolicy {
            files: FileAccess::ReadOnly,
            shell: Approval::Deny,
            network: NetworkAccess::Disabled,
        },
        false,
    );
    let err = tool_webfetch(
        &ctx,
        WebfetchArgs {
            url: "https://example.com".into(),
            method: "GET".into(),
            headers: HeaderPolicy::default(),
            redirects: RedirectPolicy::None,
            timeout_seconds: DEFAULT_WEBFETCH_TIMEOUT_SECONDS,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("tool denied by policy"));
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

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_returns_full_output_and_bounded_preview() {
    let (_dir, ctx) = test_context(auto_policy(), false);
    let value = tool_bash(
        &ctx,
        BashArgs {
            command: "python3 - <<'PY'\nprint('x' * 13000)\nPY".into(),
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    assert_eq!(value["returncode"], 0);
    assert!(value["stdout"].as_str().unwrap().len() > 12_000);
    assert!(
        value["stdout_preview"].as_str().unwrap().len() < value["stdout"].as_str().unwrap().len()
    );
    assert_eq!(value["stdout_truncated"], true);
    assert_eq!(value["stdout_capped"], false);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_output_escapes_terminal_sequences_before_returning() {
    let (_dir, ctx) = test_context(auto_policy(), false);
    let value = tool_bash(
        &ctx,
        BashArgs {
            command: "printf '\\033[31mred\\033(B\\033[m\\a\\b\\v\\f\\016\\017\\n'".into(),
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    let stdout = value["stdout"].as_str().unwrap();
    assert!(!stdout.contains('\x1b'));
    assert!(!stdout.contains('\x07'));
    assert_eq!(stdout, "␛[31mred␛(B␛[m�␈␋␌␎␏\n");
    assert_eq!(value["stdout_preview"], stdout);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bash_filters_credential_like_environment_variables() {
    let old_secret = std::env::var("OY_TEST_SECRET_TOKEN").ok();
    let old_public = std::env::var("OY_TEST_PUBLIC_VALUE").ok();
    unsafe {
        std::env::set_var("OY_TEST_SECRET_TOKEN", "do-not-leak");
        std::env::set_var("OY_TEST_PUBLIC_VALUE", "visible");
    }

    let (_dir, ctx) = test_context(auto_policy(), false);
    let value = tool_bash(
        &ctx,
        BashArgs {
            command:
                "printf '%s:%s' \"${OY_TEST_SECRET_TOKEN-unset}\" \"${OY_TEST_PUBLIC_VALUE-unset}\""
                    .into(),
            timeout_seconds: 5,
        },
    )
    .await
    .unwrap();

    match old_secret {
        Some(value) => unsafe { std::env::set_var("OY_TEST_SECRET_TOKEN", value) },
        None => unsafe { std::env::remove_var("OY_TEST_SECRET_TOKEN") },
    }
    match old_public {
        Some(value) => unsafe { std::env::set_var("OY_TEST_PUBLIC_VALUE", value) },
        None => unsafe { std::env::remove_var("OY_TEST_PUBLIC_VALUE") },
    }

    assert_eq!(value["returncode"], 0);
    assert_eq!(value["stdout"].as_str().unwrap(), "unset:visible");
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

#[test]
fn todo_tool_persists_markdown_when_requested() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolContext {
        root: dir.path().to_path_buf(),
        interactive: false,
        policy: ToolPolicy::with_write(Approval::Auto, Approval::Auto),
        todos: Vec::new(),
        external_side_effects: false,
    };
    let value = tool_todo(
        &mut ctx,
        TodoArgs {
            todos: Some(vec![TodoItemInput {
                id: Some("a".into()),
                task: "ship it".into(),
                status: TodoStatus::InProgress,
            }]),
            persist: true,
        },
    )
    .unwrap();
    assert_eq!(value["path"], TODO_FILE);
    assert_eq!(value["persisted"], true);
    let text = fs::read_to_string(dir.path().join(TODO_FILE)).unwrap();
    assert!(text.contains("- [~] a: ship it"));
}
