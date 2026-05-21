use serde_json::{Value, json};

use crate::llm::CacheHint;
use crate::llm::cache_policy::{
    Breakpoints, INLINE_BREAKPOINT_CAP, cache_point_allowed, ttl_bucket,
};

pub(crate) fn breakpoints() -> Breakpoints {
    Breakpoints::new(INLINE_BREAKPOINT_CAP)
}

pub(crate) fn block(breakpoints: &mut Breakpoints, cache: Option<&CacheHint>) -> Option<Value> {
    cache_point_allowed(breakpoints, cache).then(|| {
        match cache.and_then(|hint| hint.ttl_seconds()) {
            Some(ttl_seconds) if ttl_bucket(Some(ttl_seconds)) == Some("1h") => {
                json!({"cachePoint": {"type": "default", "ttl": "1h"}})
            }
            _ => json!({"cachePoint": {"type": "default"}}),
        }
    })
}

trait CacheHintExt {
    fn ttl_seconds(self) -> Option<u64>;
}

impl CacheHintExt for &CacheHint {
    fn ttl_seconds(self) -> Option<u64> {
        match self {
            CacheHint::Ephemeral { ttl_seconds } | CacheHint::Persistent { ttl_seconds } => {
                *ttl_seconds
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_respects_cap_and_ttl_bucket() {
        let mut breakpoints = breakpoints();
        let short = CacheHint::Ephemeral { ttl_seconds: None };
        let long = CacheHint::Persistent {
            ttl_seconds: Some(3600),
        };

        assert_eq!(block(&mut breakpoints, None), None);
        assert_eq!(
            block(&mut breakpoints, Some(&short)),
            Some(json!({"cachePoint": {"type": "default"}}))
        );
        assert_eq!(
            block(&mut breakpoints, Some(&long)),
            Some(json!({"cachePoint": {"type": "default", "ttl": "1h"}}))
        );
        assert_eq!(breakpoints.remaining, 2);
    }
}
