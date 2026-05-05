use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct RepeatedNoopTools {
    seen: BTreeSet<String>,
}

impl RepeatedNoopTools {
    pub(super) fn record(&mut self, name: &str, args: &Value, result: &Value) -> Result<()> {
        if !is_noop_tool_result(name, result) {
            self.seen.clear();
            return Ok(());
        }
        let key = format!(
            "{}:{}",
            name,
            serde_json::to_string(args).unwrap_or_default()
        );
        if !self.seen.insert(key) {
            bail!(
                "tool loop made no progress: repeated no-op {name}; inspect the latest tool output and choose a different action"
            )
        }
        Ok(())
    }
}

fn is_noop_tool_result(name: &str, result: &Value) -> bool {
    match name {
        "replace" => {
            result.get("replacement_count").and_then(Value::as_u64) == Some(0)
                && result
                    .get("changed_file_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    == 0
                && result
                    .get("errors")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repeated_noop_tools_rejects_repeated_zero_replace() {
        let mut guard = RepeatedNoopTools::default();
        let args = json!({"path": "src/main.rs", "pattern": "missing", "replacement": "x"});
        let result = json!({
            "changed_file_count": 0,
            "replacement_count": 0,
            "errors": []
        });

        guard.record("replace", &args, &result).unwrap();
        let err = guard.record("replace", &args, &result).unwrap_err();

        assert!(err.to_string().contains("repeated no-op replace"));
    }

    #[test]
    fn repeated_noop_tools_allows_retry_after_progress() {
        let mut guard = RepeatedNoopTools::default();
        let args = json!({"path": "src/main.rs", "pattern": "missing", "replacement": "x"});
        let noop = json!({
            "changed_file_count": 0,
            "replacement_count": 0,
            "errors": []
        });
        let progress = json!({
            "changed_file_count": 1,
            "replacement_count": 1,
            "errors": []
        });

        guard.record("replace", &args, &noop).unwrap();
        guard.record("replace", &args, &progress).unwrap();

        assert!(guard.record("replace", &args, &noop).is_ok());
    }
}
