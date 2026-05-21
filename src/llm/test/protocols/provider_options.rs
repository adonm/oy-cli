use super::*;
use serde_json::json;

#[test]
fn merge_json_body_rejects_non_object_options() {
    let mut body = Map::new();

    let err = merge_json_body("test-route", &mut body, Some(&json!(false))).unwrap_err();

    assert_eq!(
        err.to_string(),
        "test-route additional route params must be a JSON object"
    );
}

#[test]
fn merge_json_body_rejects_request_field_conflicts() {
    let mut body = Map::from_iter([("model".to_string(), json!("gpt-test"))]);

    let err =
        merge_json_body("test-route", &mut body, Some(&json!({"model": "override"}))).unwrap_err();

    assert_eq!(
        err.to_string(),
        "test-route additional route param `model` conflicts with the request body"
    );
}
