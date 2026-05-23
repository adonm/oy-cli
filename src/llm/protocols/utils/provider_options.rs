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
    let mut next_body = body.clone();
    for (key, value) in extra {
        merge_json_field(route, &mut next_body, key, value)?;
    }
    *body = next_body;
    Ok(())
}

fn merge_json_field(
    route: &str,
    body: &mut Map<String, Value>,
    key: &str,
    value: &Value,
) -> Result<()> {
    if let Some(existing) = body.get_mut(key) {
        if existing.is_object() && value.is_object() {
            merge_json_objects(route, key, existing, value.clone())?;
            return Ok(());
        }
        bail!("{route} additional route param `{key}` conflicts with the request body");
    }
    body.insert(key.to_string(), value.clone());
    Ok(())
}

fn merge_json_objects(route: &str, path: &str, base: &mut Value, overlay: Value) -> Result<()> {
    let Some(base_object) = base.as_object_mut() else {
        bail!("{route} additional route param `{path}` conflicts with the request body");
    };
    let Value::Object(overlay) = overlay else {
        bail!("{route} additional route param `{path}` conflicts with the request body");
    };
    for (key, value) in overlay {
        let child_path = format!("{path}.{key}");
        match (base_object.get_mut(&key), value) {
            (Some(existing), Value::Object(next)) if existing.is_object() => {
                merge_json_objects(route, &child_path, existing, Value::Object(next))?;
            }
            (Some(_), _) => {
                bail!(
                    "{route} additional route param `{child_path}` conflicts with the request body"
                );
            }
            (_, value) => {
                base_object.insert(key, value);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../test/protocols/provider_options.rs"]
mod tests;
