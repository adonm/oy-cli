use backon::ExponentialBuilder;

const TRANSIENT_RETRY_ATTEMPTS: usize = 10;

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
    // Rig parses successful OpenAI-compatible responses through an untagged
    // ApiResponse enum. Some proxies occasionally return a transient 2xx body
    // that is neither a chat completion nor Rig's top-level error shape; retry
    // those through the normal LLM backoff instead of failing immediately.
    "data did not match any variant of untagged enum ApiResponse",
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
/// 60s maximum delay, no jitter) with a project-wide 10 retry attempts.
pub fn llm_backoff() -> ExponentialBuilder {
    ExponentialBuilder::default().with_max_times(TRANSIENT_RETRY_ATTEMPTS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use backon::BackoffBuilder;
    use std::time::Duration;

    #[test]
    fn llm_backoff_uses_backon_defaults_with_ten_attempts() {
        let delays: Vec<_> = llm_backoff().build().collect();

        assert_eq!(delays.len(), 10);
        assert_eq!(delays[0], Duration::from_secs(1));
        assert_eq!(delays[1], Duration::from_secs(2));
        assert_eq!(delays[2], Duration::from_secs(4));
        assert_eq!(delays[6], Duration::from_secs(60));
    }

    #[test]
    fn rig_api_response_parse_failures_are_retried() {
        let err = anyhow::anyhow!(
            "CompletionError: JsonError: data did not match any variant of untagged enum ApiResponse"
        );

        assert!(is_transient_error(&err));
    }
}
