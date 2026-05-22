use super::*;

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
    assert_eq!(ctx.todos()[0].task, "plan work");

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
    assert_eq!(ctx.todos().len(), 1);

    let read = invoke(&mut ctx, "todo", json!({})).await.unwrap();
    assert_eq!(read["count"], 1);
    assert_eq!(ctx.todos().len(), 1);

    let cleared = invoke(&mut ctx, "todo", json!({ "todos": [] }))
        .await
        .unwrap();
    assert_eq!(cleared["count"], 0);
    assert!(ctx.todos().is_empty());
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
    assert_eq!(ctx.todos()[0].task, "canonical");
}

#[test]
fn todo_tool_persists_markdown_when_requested() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = ToolContext::new(
        dir.path().to_path_buf(),
        false,
        ToolPolicy::with_write(Approval::Auto, Approval::Auto),
        Vec::new(),
    );
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
