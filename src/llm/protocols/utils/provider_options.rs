use anyhow::{Result, bail};
use serde_json::{Map, Value};

pub(crate) fn merge_json_body(
    route: &str,
    body: &mut Map<String, Value>,
    additional_params: Option<&Value>,
) -> Result<()> {
    let Some(additional_params) = additional_params else {
        return Ok(());
    };
    let Value::Object(extra) = additional_params else {
        bail!("{route} additional route params must be a JSON object");
    };
    for (key, value) in extra {
        if body.contains_key(key) {
            bail!("{route} additional route param `{key}` conflicts with the request body");
        }
        body.insert(key.clone(), value.clone());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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

        let err = merge_json_body("test-route", &mut body, Some(&json!({"model": "override"})))
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "test-route additional route param `model` conflicts with the request body"
        );
    }
}
