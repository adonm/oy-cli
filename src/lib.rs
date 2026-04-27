#![recursion_limit = "256"]

mod app;
mod audit;
mod bedrock;
mod chat;
mod config;
mod model;
mod prompts;
mod session;
mod tools;
mod ui;

pub use ui::{OutputMode, set_output_mode};

pub async fn run(argv: Vec<String>) -> anyhow::Result<i32> {
    app::run(argv).await
}

pub fn chat_help_text() -> String {
    chat::chat_help_text()
}

pub fn preview_tool_output(name: &str, value: &serde_json::Value) -> String {
    tools::preview_tool_output(name, value)
}

pub fn err_line(args: std::fmt::Arguments<'_>) {
    ui::err_line(args);
}
