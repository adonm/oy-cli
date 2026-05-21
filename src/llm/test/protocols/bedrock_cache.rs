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
