use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{DEFAULT_LIMIT, DEFAULT_WEBFETCH_TIMEOUT_SECONDS};

pub(super) fn object<const N: usize>(properties: [(&str, Value); N], required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, schema) in properties {
        props.insert(name.to_string(), schema);
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), Value::Object(props));
    schema.insert("additionalProperties".to_string(), json!(false));
    if !required.is_empty() {
        schema.insert("required".to_string(), json!(required));
    }
    Value::Object(schema)
}

pub(super) fn string() -> Value {
    json!({"type": "string"})
}

pub(super) fn string_default(default: &str) -> Value {
    json!({"type": "string", "default": default})
}

pub(super) fn string_enum(values: &[&str], default: &str) -> Value {
    json!({"type": "string", "enum": values, "default": default})
}

pub(super) fn integer_default(default: impl Serialize) -> Value {
    json!({"type": ["integer", "string"], "default": default})
}

pub(super) fn bool_default(default: bool) -> Value {
    json!({"type": "boolean", "default": default})
}

pub(super) fn array_of(items: Value) -> Value {
    json!({"type": "array", "items": items})
}

pub(super) fn nullable_string_array() -> Value {
    json!({"type": ["array", "null"], "items": string()})
}

pub(super) fn describe(mut schema: Value, description: &str) -> Value {
    schema["description"] = json!(description);
    schema
}

pub(super) fn exclude_schema() -> Value {
    json!({"anyOf": [string(), array_of(string()), {"type": "null"}]})
}

pub(super) fn todo_item_schema() -> Value {
    object(
        [
            (
                "id",
                describe(
                    string(),
                    "Stable short id; optional, defaults to 1-based position.",
                ),
            ),
            ("task", string()),
            (
                "status",
                string_enum(&["pending", "in_progress", "done"], "pending"),
            ),
        ],
        &["task"],
    )
}

pub(super) fn schema_list() -> Value {
    object(
        [
            ("path", string_default("*")),
            ("exclude", exclude_schema()),
            ("limit", integer_default(DEFAULT_LIMIT)),
        ],
        &[],
    )
}

pub(super) fn schema_read() -> Value {
    object(
        [
            ("path", string()),
            ("offset", integer_default(1)),
            ("limit", integer_default(DEFAULT_LIMIT)),
        ],
        &["path"],
    )
}

pub(super) fn schema_search() -> Value {
    object(
        [
            ("pattern", string()),
            ("path", string_default(".")),
            ("exclude", exclude_schema()),
            ("limit", integer_default(DEFAULT_LIMIT)),
            ("mode", string_enum(&["auto", "regex", "literal"], "auto")),
        ],
        &["pattern"],
    )
}

pub(super) fn schema_sloc() -> Value {
    object(
        [
            (
                "path",
                describe(
                    string_default("."),
                    "Workspace path or whitespace-separated paths to count.",
                ),
            ),
            ("exclude", exclude_schema()),
        ],
        &[],
    )
}

pub(super) fn schema_todo() -> Value {
    let item = todo_item_schema();
    object(
        [
            (
                "todos",
                describe(
                    array_of(item.clone()),
                    "Complete replacement todo list. Alias: items. Omit to return current list.",
                ),
            ),
            ("items", describe(array_of(item), "Alias for todos.")),
            (
                "persist",
                describe(
                    bool_default(false),
                    "Write to TODO.md; default false avoids git churn.",
                ),
            ),
        ],
        &[],
    )
}

pub(super) fn schema_ask() -> Value {
    object(
        [("question", string()), ("choices", nullable_string_array())],
        &["question"],
    )
}

pub(super) fn schema_webfetch() -> Value {
    object(
        [
            ("url", string()),
            ("method", string_default("GET")),
            (
                "headers",
                json!({"type": ["object", "null"], "additionalProperties": string()}),
            ),
            ("follow_redirects", bool_default(true)),
            (
                "timeout_seconds",
                integer_default(DEFAULT_WEBFETCH_TIMEOUT_SECONDS),
            ),
        ],
        &["url"],
    )
}

pub(super) fn schema_replace() -> Value {
    object(
        [
            ("pattern", string()),
            ("replacement", string()),
            ("path", string_default(".")),
            ("exclude", exclude_schema()),
            ("limit", integer_default(DEFAULT_LIMIT)),
            ("mode", string_enum(&["regex", "literal"], "regex")),
        ],
        &["pattern", "replacement"],
    )
}

pub(super) fn schema_bash() -> Value {
    object(
        [
            ("command", string()),
            ("timeout_seconds", integer_default(120)),
        ],
        &["command"],
    )
}
