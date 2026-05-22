use super::*;

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
        "Complete replacement todo list; this replaces all existing todo items. Alias: items. Omit to return current list."
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

    let scrape = schema_for("webfetch");
    assert_eq!(scrape["required"], json!(["url"]));
    assert_eq!(scrape["properties"]["return_format"]["default"], "markdown");
    assert_eq!(
        scrape["properties"]["return_format"]["enum"],
        json!(["raw", "markdown", "text", "xml"])
    );
    assert!(scrape["properties"].get("proxy").is_none());
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
