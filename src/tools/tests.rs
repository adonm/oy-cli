use super::args::{
    BashArgs, ExcludeArg, ListArgs, PatchArgs, ReadArgs, ReplaceArgs, ReplaceMode, ReturnFormat,
    SearchArgs, SearchMode, SlocArgs, TodoArgs, TodoItemInput, WebfetchArgs,
};
use super::network::tool_webfetch;
use super::todo::tool_todo;
use super::workspace;
use super::*;
use serde_json::{Value, json};
use std::fs;

fn test_context(policy: ToolPolicy, interactive: bool) -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path().to_path_buf(), interactive, policy, Vec::new());
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

mod network;
mod policy;
mod schema;
mod shell;
mod todo;
mod list;
mod patch;
mod read;
mod replace;
mod search;
mod sloc;
