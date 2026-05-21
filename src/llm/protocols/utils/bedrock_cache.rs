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
#[path = "../../test/protocols/bedrock_cache.rs"]
mod tests;
