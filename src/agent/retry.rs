//! Transient-error detection and jittered exponential backoff for
//! LLM API retries.
//!
//! The native backend applies this at each provider HTTP/streaming call before
//! local tool execution for that turn. The backoff budget is small
//! (4 attempts, 1s–60s range) with jitter.

use backon::ExponentialBuilder;

const TRANSIENT_RETRY_ATTEMPTS: usize = 4;
#[cfg(test)]
const RETRY_JITTER_SEED: u64 = 0x0bad_f00d;

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

/// Exponential backoff for transient LLM API failures.
///
/// Uses [`backon::ExponentialBuilder`] defaults (1s minimum delay, factor 2,
/// 60s maximum delay) with jitter and a small project-wide retry budget.
pub fn llm_backoff() -> ExponentialBuilder {
    ExponentialBuilder::default()
        .with_max_times(TRANSIENT_RETRY_ATTEMPTS)
        .with_jitter()
}

#[cfg(test)]
fn deterministic_llm_backoff() -> ExponentialBuilder {
    ExponentialBuilder::default()
        .with_max_times(TRANSIENT_RETRY_ATTEMPTS)
        .with_jitter()
        .with_jitter_seed(RETRY_JITTER_SEED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backon::BackoffBuilder;
    use std::time::Duration;

    #[test]
    fn llm_backoff_uses_jittered_four_attempt_budget() {
        let delays: Vec<_> = deterministic_llm_backoff().build().collect();

        assert_eq!(delays.len(), 4);
        assert!(delays[0] >= Duration::from_secs(1));
        assert!(delays[0] < Duration::from_secs(2));
        assert!(delays[1] >= Duration::from_secs(2));
        assert!(delays[1] < Duration::from_secs(4));
        assert!(delays[2] >= Duration::from_secs(4));
        assert!(delays[2] < Duration::from_secs(8));
    }
}
