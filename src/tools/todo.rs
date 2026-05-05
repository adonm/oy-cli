use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::args::TodoArgs;
use super::{
    PREVIEW_ITEMS, TODO_FILE, TodoItem, TodoStatus, ToolContext, preview, require_mutation_approval,
};

// === Todo formatting and persistence ===
pub(crate) fn format_todos(todos: &[TodoItem]) -> String {
    if todos.is_empty() {
        return "<empty todo list>".to_string();
    }
    todos
        .iter()
        .map(format_todo_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_todo_preview(todos: &[TodoItem]) -> String {
    let counts = todo_status_counts(todos);
    let active = counts.pending + counts.in_progress;
    let mut out = format!(
        "{} todo{} · {} active · {} done",
        todos.len(),
        preview::plural(todos.len()),
        active,
        counts.done
    );
    let lines = format_todos(todos);
    let total_lines = lines.lines().count();
    for line in lines.lines().take(PREVIEW_ITEMS) {
        let _ = write!(out, "\n  {line}");
    }
    if total_lines > PREVIEW_ITEMS {
        let _ = write!(out, "\n  … {} more todos", total_lines - PREVIEW_ITEMS);
    }
    out
}

pub(super) fn format_todo_preview_from_values(items: &[Value]) -> String {
    let todos = items
        .iter()
        .filter_map(|item| serde_json::from_value(item.clone()).ok())
        .collect::<Vec<_>>();
    format_todo_preview(&todos)
}

fn format_todo_line(item: &TodoItem) -> String {
    let icon = match item.status {
        TodoStatus::Done => "✓",
        TodoStatus::InProgress => "…",
        TodoStatus::Pending => "·",
    };
    if item.task.is_empty() {
        format!("{icon} {}", item.id)
    } else {
        format!("{icon} {} {}", item.id, item.task)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(super) struct TodoStatusCounts {
    pending: usize,
    in_progress: usize,
    done: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct TodoOutput {
    pub path: &'static str,
    pub persisted: bool,
    pub items: Vec<TodoItem>,
    pub count: usize,
    pub status_counts: TodoStatusCounts,
    pub preview: String,
}

fn todo_status_counts(todos: &[TodoItem]) -> TodoStatusCounts {
    todos
        .iter()
        .fold(TodoStatusCounts::default(), |mut counts, item| {
            match item.status {
                TodoStatus::Done => counts.done += 1,
                TodoStatus::InProgress => counts.in_progress += 1,
                TodoStatus::Pending => counts.pending += 1,
            }
            counts
        })
}

fn save_todos_to_file(root: &Path, todos: &[TodoItem]) -> Result<()> {
    let path = todo_path(root);
    crate::config::write_workspace_file(&path, todos_to_markdown(todos).as_bytes())
        .with_context(|| format!("failed to write {}", TODO_FILE))
}

fn todo_path(root: &Path) -> PathBuf {
    root.join(TODO_FILE)
}

fn todos_to_markdown(todos: &[TodoItem]) -> String {
    let mut out = String::from(
        "# todo

",
    );
    if todos.is_empty() {
        out.push_str(
            "<!-- empty -->
",
        );
        return out;
    }
    for item in todos {
        let box_mark = match item.status {
            TodoStatus::Done => "x",
            TodoStatus::InProgress => "~",
            TodoStatus::Pending => " ",
        };
        let _ = writeln!(out, "- [{box_mark}] {}: {}", item.id, item.task);
    }
    out
}

pub(super) fn tool_todo(ctx: &mut ToolContext, args: TodoArgs) -> Result<Value> {
    if !args.todos.is_empty() {
        require_mutation_approval(ctx, "todo", Some("update the in-memory todo list"))?;
    }
    if args.persist {
        require_mutation_approval(ctx, "todo_persist", Some("write TODO.md in the workspace"))?;
    }
    let input_todos = if args.todos.is_empty() {
        ctx.todos.clone()
    } else {
        args.todos
            .into_iter()
            .map(|item| TodoItem {
                id: item.id.unwrap_or_default(),
                task: item.task,
                status: item.status,
            })
            .collect()
    };
    let todos = input_todos
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let id = Some(crate::ui::compact_spaces(&item.id))
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| (index + 1).to_string());
            let task = crate::ui::compact_spaces(&item.task);
            if task.is_empty() {
                bail!("todo task cannot be empty");
            }
            Ok(TodoItem {
                id,
                task,
                status: item.status,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ctx.todos = todos;
    if args.persist {
        save_todos_to_file(&ctx.root, &ctx.todos)?;
    }
    let counts = todo_status_counts(&ctx.todos);
    let preview = format_todo_preview(&ctx.todos);
    Ok(serde_json::to_value(TodoOutput {
        path: TODO_FILE,
        persisted: args.persist,
        items: ctx.todos.clone(),
        count: ctx.todos.len(),
        status_counts: counts,
        preview,
    })?)
}
