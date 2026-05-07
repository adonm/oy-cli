use backon::ExponentialBuilder;
use std::time::Duration;

const STATUS_RETRY_PATTERNS: &[&str] = &[
    "500",
    "502",
    "503",
    "504",
    "429",
    "Internal Server Error",
    "Bad Gateway",
    "Service Unavailable",
    "Gateway Timeout",
    "Too Many Requests",
    "rate limit",
    "Rate limit",
    "timed out",
    "connection closed",
    "connection refused",
    "connection reset",
    "broken pipe",
    "incomplete message",
    "unexpected EOF",
    "request timeout",
    "dns error",
    "network error",
];

/// Returns `true` when the error chain contains a transient failure worth
/// retrying (server errors, rate limits, network timeouts, etc.).
pub fn is_transient_error(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        let text = cause.to_string();
        for pattern in STATUS_RETRY_PATTERNS {
            if text.contains(pattern) {
                return true;
            }
        }
    }
    false
}

/// Exponential backoff tuned for LLM API retries.
///
/// Uses [`backon::ExponentialBuilder`] defaults:
/// 1s → 2s → 4s, factor 2, max 3 retry attempts, no jitter.
pub fn llm_backoff() -> ExponentialBuilder {
    ExponentialBuilder::default().with_min_delay(Duration::from_millis(500))
}
