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
#[path = "../../test/protocols/provider_options.rs"]
mod tests;
