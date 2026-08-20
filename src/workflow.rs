//! Shared workflow identifiers for prepared artifact runs.

use anyhow::Result;

/// Generate a hex run ID for a prepared artifact run.
pub(crate) fn new_run_id() -> Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("failed generating workflow run id: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_unique_hex_strings() {
        let first = new_run_id().unwrap();
        let second = new_run_id().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 48);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(second.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
