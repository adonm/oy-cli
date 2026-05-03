use anyhow::Result;
use chrono::{DateTime, Utc};
use genai::chat::{ChatOptions, ChatRequest, ChatResponse};
use genai::webc;
use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;

const CHAT_RATE_LIMIT_MAX_RETRIES: usize = 3;
const CHAT_RATE_LIMIT_DEFAULT_DELAY: Duration = Duration::from_secs(2);
const CHAT_RATE_LIMIT_MAX_DELAY: Duration = Duration::from_secs(60);

pub(super) fn display_model(model_spec: &str) -> &str {
    model_spec
        .rsplit_once("::")
        .map(|(_, model)| model)
        .unwrap_or(model_spec)
}

pub(super) fn token_count_text(count: usize) -> String {
    if count < 1000 {
        format!("{count} tok")
    } else {
        format!("{:.1}k tok", count as f64 / 1000.0)
    }
}

pub(super) async fn exec_chat(
    model_spec: &str,
    client: &genai::Client,
    req: ChatRequest,
    options: Option<&ChatOptions>,
) -> Result<ChatResponse> {
    let retry_label = display_model(model_spec).to_string();
    retry_rate_limited_chat(&retry_label, || {
        let req = req.clone();
        let options = options.cloned();
        async move {
            if crate::bedrock::is_bedrock_model(model_spec) {
                crate::bedrock::exec_chat(model_spec, req, options.as_ref()).await
            } else {
                Ok(client.exec_chat(model_spec, req, options.as_ref()).await?)
            }
        }
    })
    .await
}

async fn retry_rate_limited_chat<F, Fut>(label: &str, mut call: F) -> Result<ChatResponse>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<ChatResponse>>,
{
    let mut attempt = 0usize;
    loop {
        match call().await {
            Ok(response) => return Ok(response),
            Err(err) => {
                let Some(delay) = rate_limit_retry_delay(err.as_ref(), attempt) else {
                    return Err(err);
                };
                attempt += 1;
                if !crate::ui::is_quiet() {
                    crate::ui::err_line(format_args!(
                        "oy · {label} · rate limited; retrying in {}s ({attempt}/{CHAT_RATE_LIMIT_MAX_RETRIES})",
                        delay.as_secs()
                    ));
                }
                sleep(delay).await;
            }
        }
    }
}

fn rate_limit_retry_delay(
    err: &(dyn std::error::Error + 'static),
    attempt: usize,
) -> Option<Duration> {
    if attempt >= CHAT_RATE_LIMIT_MAX_RETRIES {
        return None;
    }

    genai_rate_limit_delay(err)
        .or_else(|| bedrock_rate_limit_delay(err))
        .map(|delay| delay.clamp(Duration::from_secs(1), CHAT_RATE_LIMIT_MAX_DELAY))
}

fn genai_rate_limit_delay(err: &(dyn std::error::Error + 'static)) -> Option<Duration> {
    let err = err.downcast_ref::<genai::Error>()?;
    match err {
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => webc_rate_limit_delay(webc_error),
        genai::Error::HttpError { status, .. } if *status == StatusCode::TOO_MANY_REQUESTS => {
            Some(CHAT_RATE_LIMIT_DEFAULT_DELAY)
        }
        _ => None,
    }
}

fn webc_rate_limit_delay(err: &webc::Error) -> Option<Duration> {
    let webc::Error::ResponseFailedStatus {
        status, headers, ..
    } = err
    else {
        return None;
    };
    if *status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
        .or(Some(CHAT_RATE_LIMIT_DEFAULT_DELAY))
}

fn bedrock_rate_limit_delay(err: &(dyn std::error::Error + 'static)) -> Option<Duration> {
    let text = err.to_string().to_ascii_lowercase();
    if text.contains("throttling")
        || text.contains("too many requests")
        || text.contains("rate exceeded")
    {
        Some(CHAT_RATE_LIMIT_DEFAULT_DELAY)
    } else {
        None
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let retry_at = DateTime::parse_from_rfc2822(value).ok()?;
    let delay = retry_at
        .with_timezone(&Utc)
        .signed_duration_since(Utc::now());
    delay.to_std().ok().or(Some(Duration::from_secs(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use genai::ModelIden;
    use genai::adapter::AdapterKind;

    #[test]
    fn rate_limit_delay_respects_retry_after_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, "7".parse().unwrap());
        let err = genai::Error::WebModelCall {
            model_iden: ModelIden::new(AdapterKind::OpenAI, "gpt-test"),
            webc_error: webc::Error::ResponseFailedStatus {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: "rate limited".into(),
                headers: Box::new(headers),
            },
        };

        assert_eq!(
            rate_limit_retry_delay(&err, 0),
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn rate_limit_delay_clamps_retry_after_and_retry_count() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, "120".parse().unwrap());
        let err = genai::Error::WebAdapterCall {
            adapter_kind: AdapterKind::OpenAI,
            webc_error: webc::Error::ResponseFailedStatus {
                status: StatusCode::TOO_MANY_REQUESTS,
                body: "rate limited".into(),
                headers: Box::new(headers),
            },
        };

        assert_eq!(
            rate_limit_retry_delay(&err, 0),
            Some(CHAT_RATE_LIMIT_MAX_DELAY)
        );
        assert_eq!(
            rate_limit_retry_delay(&err, CHAT_RATE_LIMIT_MAX_RETRIES),
            None
        );
    }

    #[test]
    fn rate_limit_delay_ignores_non_429_status() {
        let err = genai::Error::WebModelCall {
            model_iden: ModelIden::new(AdapterKind::OpenAI, "gpt-test"),
            webc_error: webc::Error::ResponseFailedStatus {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: "server error".into(),
                headers: Box::new(reqwest::header::HeaderMap::new()),
            },
        };

        assert_eq!(rate_limit_retry_delay(&err, 0), None);
    }
}
