use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{DEFAULT_LIMIT, DEFAULT_WEBFETCH_TIMEOUT_SECONDS};

// === Schema builder ===

/// A JSON Schema value under construction.
#[derive(Debug, Clone)]
pub(super) struct Schema(pub(super) Value);

impl Schema {
    /// Return a closed object builder.
    pub fn object() -> ObjectBuilder {
        ObjectBuilder::default()
    }

    pub fn string() -> Self {
        Self(json!({"type": "string"}))
    }

    pub fn integer() -> Self {
        Self(json!({"type": ["integer", "string"]}))
    }

    pub fn boolean() -> Self {
        Self(json!({"type": "boolean"}))
    }

    pub fn array(items: Schema) -> Self {
        Self(json!({"type": "array", "items": items.0}))
    }

    pub fn null() -> Self {
        Self(json!({"type": "null"}))
    }

    /// AnyOf combinator for nullable or union types.
    pub fn any_of(schemas: Vec<Schema>) -> Self {
        let items: Vec<Value> = schemas.into_iter().map(|s| s.0).collect();
        Self(json!({"anyOf": items}))
    }

    /// Nullable object with additional properties (for headers-style schemas).
    pub fn nullable_open_object(additional: Schema) -> Self {
        Self(json!({"type": ["object", "null"], "additionalProperties": additional.0}))
    }

    /// Attach a default value.
    pub fn default(mut self, value: impl Serialize) -> Self {
        self.0["default"] = json!(value);
        self
    }

    /// Attach an enum constraint.
    pub fn enum_values(mut self, values: &[&str]) -> Self {
        self.0["enum"] = json!(values);
        self
    }

    /// Attach a description.
    pub fn describe(mut self, text: &str) -> Self {
        self.0["description"] = json!(text);
        self
    }
}

/// Builder for a JSON Schema object with properties.
#[derive(Default)]
pub(super) struct ObjectBuilder {
    properties: Map<String, Value>,
    required: Vec<String>,
}

impl ObjectBuilder {
    /// Add a property.
    pub fn property(mut self, name: &str, schema: Schema) -> Self {
        self.properties.insert(name.to_string(), schema.0);
        self
    }

    /// Mark previously added properties as required.
    pub fn required(mut self, names: &[&str]) -> Self {
        self.required.extend(names.iter().map(|s| s.to_string()));
        self
    }

    /// Build the closed object schema as a raw Value.
    pub fn build(self) -> Value {
        let mut schema = Map::new();
        schema.insert("type".to_string(), json!("object"));
        schema.insert("properties".to_string(), Value::Object(self.properties));
        schema.insert("additionalProperties".to_string(), json!(false));
        if !self.required.is_empty() {
            schema.insert("required".to_string(), json!(self.required));
        }
        Value::Object(schema)
    }

    /// Build the closed object schema as a Schema value.
    pub fn build_schema(self) -> Schema {
        Schema(self.build())
    }
}

// === Shared sub-schemas ===

fn exclude_schema() -> Schema {
    Schema::any_of(vec![
        Schema::string(),
        Schema::array(Schema::string()),
        Schema::null(),
    ])
}

fn todo_item_schema() -> Schema {
    Schema::object()
        .property(
            "id",
            Schema::string().describe("Stable short id; optional, defaults to 1-based position."),
        )
        .property("task", Schema::string())
        .property(
            "status",
            Schema::string()
                .enum_values(&["pending", "in_progress", "done"])
                .default("pending"),
        )
        .required(&["task"])
        .build_schema()
}

// === Per-tool schema functions ===

pub(super) fn schema_list() -> Value {
    Schema::object()
        .property("path", Schema::string().default("*"))
        .property("exclude", exclude_schema())
        .property("limit", Schema::integer().default(DEFAULT_LIMIT))
        .build()
}

pub(super) fn schema_read() -> Value {
    Schema::object()
        .property("path", Schema::string())
        .property("offset", Schema::integer().default(1))
        .property("limit", Schema::integer().default(DEFAULT_LIMIT))
        .required(&["path"])
        .build()
}

pub(super) fn schema_search() -> Value {
    Schema::object()
        .property("pattern", Schema::string())
        .property("path", Schema::string().default("."))
        .property("exclude", exclude_schema())
        .property("limit", Schema::integer().default(DEFAULT_LIMIT))
        .property(
            "mode",
            Schema::string()
                .enum_values(&["auto", "regex", "literal"])
                .default("auto"),
        )
        .required(&["pattern"])
        .build()
}

pub(super) fn schema_sloc() -> Value {
    Schema::object()
        .property(
            "path",
            Schema::string()
                .default(".")
                .describe("Workspace path or whitespace-separated paths to count."),
        )
        .property("exclude", exclude_schema())
        .build()
}

pub(super) fn schema_todo() -> Value {
    let item = todo_item_schema();
    Schema::object()
        .property(
            "todos",
            Schema::array(item.clone()).describe(
                "Complete replacement todo list. Alias: items. Omit to return current list.",
            ),
        )
        .property("items", Schema::array(item).describe("Alias for todos."))
        .property(
            "persist",
            Schema::boolean()
                .default(false)
                .describe("Write to TODO.md; default false avoids git churn."),
        )
        .build()
}

pub(super) fn schema_ask() -> Value {
    Schema::object()
        .property("question", Schema::string())
        .property(
            "choices",
            Schema::any_of(vec![Schema::array(Schema::string()), Schema::null()]),
        )
        .required(&["question"])
        .build()
}

pub(super) fn schema_webfetch() -> Value {
    Schema::object()
        .property("url", Schema::string())
        .property("method", Schema::string().default("GET"))
        .property("headers", Schema::nullable_open_object(Schema::string()))
        .property("follow_redirects", Schema::boolean().default(true))
        .property(
            "timeout_seconds",
            Schema::integer().default(DEFAULT_WEBFETCH_TIMEOUT_SECONDS),
        )
        .required(&["url"])
        .build()
}

pub(super) fn schema_replace() -> Value {
    Schema::object()
        .property("pattern", Schema::string())
        .property("replacement", Schema::string())
        .property("path", Schema::string().default("."))
        .property("exclude", exclude_schema())
        .property("limit", Schema::integer().default(DEFAULT_LIMIT))
        .property(
            "mode",
            Schema::string()
                .enum_values(&["regex", "literal"])
                .default("regex"),
        )
        .required(&["pattern", "replacement"])
        .build()
}

pub(super) fn schema_patch() -> Value {
    Schema::object()
        .property(
            "patch",
            Schema::string().describe(
                "Unified or git diff to apply. Existing UTF-8 files only; create/delete/rename/copy/binary patches are rejected.",
            ),
        )
        .property("strip", Schema::integer().default(1).describe("Path components to strip, like patch -p. Git diffs usually use 1."))
        .property("limit", Schema::integer().default(DEFAULT_LIMIT))
        .required(&["patch"])
        .build()
}

pub(super) fn schema_bash() -> Value {
    Schema::object()
        .property("command", Schema::string())
        .property("timeout_seconds", Schema::integer().default(120))
        .required(&["command"])
        .build()
}
