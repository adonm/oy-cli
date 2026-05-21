use super::*;
use serde_json::json;

#[test]
fn tool_spec_serializes_without_backend_details() {
    let spec = ToolSpec {
        name: "read".to_string(),
        description: "Read one file".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false
        }),
        cache: None,
    };

    let actual = serde_json::to_string_pretty(&spec).unwrap();
    let expected = r#"{
  "name": "read",
  "description": "Read one file",
  "parameters": {
    "type": "object",
    "properties": {
      "path": {
        "type": "string"
      }
    },
    "required": [
      "path"
    ],
    "additionalProperties": false
  }
}"#;
    assert_eq!(actual, expected);
}
