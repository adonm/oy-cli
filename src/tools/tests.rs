use super::args::{
    BashArgs, HeaderPolicy, ListArgs, ReadArgs, RedirectPolicy, ReplaceArgs, ReplaceMode,
    SearchArgs, SearchMode, SlocArgs, TodoArgs, TodoItemInput, WebfetchArgs,
};
use super::network::{is_public_ip, validated_webfetch_headers};
use super::todo::tool_todo;
use super::workspace::{self, search_file};
use super::*;
use grep_regex::RegexMatcher;
use regex::Regex;
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
        .and_then(|tool| tool.schema)
        .unwrap_or_else(|| panic!("missing schema for {name}"))
}

#[test]
fn tool_schemas_are_closed_objects_with_valid_required_fields() {
    let (_dir, ctx) = test_context(auto_policy(), true);
    for tool in tool_specs(&ctx) {
        let schema = tool
            .schema
            .unwrap_or_else(|| panic!("missing schema for {}", tool.name));
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
        "127.0.0.1",
        "10.0.0.1",
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
    ] {
        assert!(!is_public_ip(ip.parse().unwrap()), "{ip} should be denied");
    }
    for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
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

    let bash = schema_for("bash");
    assert_eq!(
        bash["properties"]["timeout_seconds"]["type"],
        json!(["integer", "string"])
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
            todos: vec![TodoItemInput {
                id: None,
                task: "plan work".into(),
                status: TodoStatus::Pending,
            }],
            persist: false,
        },
    )
    .unwrap();
    assert_eq!(value["count"], 1);
    assert_eq!(ctx.todos[0].task, "plan work");

    let err = tool_todo(
        &mut ctx,
        TodoArgs {
            todos: Vec::new(),
            persist: true,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("tool denied by policy"));
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
    for denied in ["replace", "bash"] {
        assert!(
            !names.iter().any(|name| name.as_str() == denied),
            "exposed {denied}"
        );
    }
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
    let dir = tempfile::tempdir().unwrap();
    let zip_path = dir.path().join("sample.zip");
    fs::write(&zip_path, b"PK\0\0not searched").unwrap();
    let matcher = RegexMatcher::new_line_matcher("not searched").unwrap();
    let column_regex = Regex::new("not searched").unwrap();
    let found = search_file(dir.path(), &zip_path, &matcher, &column_regex).unwrap();
    assert!(found.is_empty());
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

#[test]
fn todo_tool_persists_markdown_when_requested() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolContext {
        root: dir.path().to_path_buf(),
        interactive: false,
        policy: ToolPolicy::with_write(Approval::Auto, Approval::Auto),
        todos: Vec::new(),
    };
    let value = tool_todo(
        &mut ctx,
        TodoArgs {
            todos: vec![TodoItemInput {
                id: Some("a".into()),
                task: "ship it".into(),
                status: TodoStatus::InProgress,
            }],
            persist: true,
        },
    )
    .unwrap();
    assert_eq!(value["path"], TODO_FILE);
    assert_eq!(value["persisted"], true);
    let text = fs::read_to_string(dir.path().join(TODO_FILE)).unwrap();
    assert!(text.contains("- [~] a: ship it"));
}
