use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;

use super::{auth, framing::SseDecoder};
use crate::llm::RouteAuth;
use crate::llm::schema::{LlmEvent, StepAccumulator};

pub(crate) async fn post_json_streaming(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<reqwest::Response> {
    let response = post_json_request(client, endpoint, auth, body)?
        .send()
        .await
        .with_context(|| format!("failed to send native OpenAI request to {endpoint}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response
            .text()
            .await
            .context("failed to read native OpenAI error response body")?;
        bail!(
            "native OpenAI request failed ({}): {}",
            status,
            provider_error_message(status, &text)
        );
    }
    Ok(response)
}

pub(crate) async fn post_bedrock_json_streaming(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<reqwest::Response> {
    let response = post_json_request(client, endpoint, auth, body)?
        .send()
        .await
        .with_context(|| format!("failed to send native Bedrock request to {endpoint}"))?;
    let status = response.status();
    if !status.is_success() {
        let text = response
            .text()
            .await
            .context("failed to read native Bedrock error response body")?;
        bail!(
            "native Bedrock request failed ({}): {}",
            status,
            provider_error_message(status, &text)
        );
    }
    Ok(response)
}

fn post_json_request(
    client: &reqwest::Client,
    endpoint: &str,
    auth: &RouteAuth,
    body: &Value,
) -> Result<reqwest::RequestBuilder> {
    let body_text =
        serde_json::to_string(body).context("failed to encode native LLM request body")?;
    let builder = client.post(endpoint).body(body_text.clone());
    auth::apply_json_headers(builder, auth, endpoint, &body_text)
}

pub(crate) async fn stream_json_sse_events<State>(
    response: reqwest::Response,
    mut state: State,
    mut handle_event: impl FnMut(&mut State, Value) -> Result<Vec<LlmEvent>>,
    finish: impl FnOnce(&mut State) -> Result<Vec<LlmEvent>>,
) -> Result<StepAccumulator> {
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut step = StepAccumulator::default();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("failed to read native OpenAI SSE chunk")?;
        for data in decoder.push_chunk(&chunk) {
            consume_json_event(&data, &mut state, &mut handle_event, &mut step)?;
        }
    }

    for data in decoder.finish() {
        consume_json_event(&data, &mut state, &mut handle_event, &mut step)?;
    }

    for event in finish(&mut state)? {
        step.push(event)?;
    }

    Ok(step)
}

fn consume_json_event<State>(
    data: &str,
    state: &mut State,
    handle_event: &mut impl FnMut(&mut State, Value) -> Result<Vec<LlmEvent>>,
    step: &mut StepAccumulator,
) -> Result<()> {
    let event: Value = serde_json::from_str(data)
        .with_context(|| format!("failed to parse native OpenAI SSE event: {data}"))?;
    for event in handle_event(state, event)? {
        step.push(event)?;
    }
    Ok(())
}

fn provider_error_message(status: StatusCode, text: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(message) = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
    {
        return message.to_string();
    }
    let text = text.trim();
    if text.is_empty() {
        status
            .canonical_reason()
            .unwrap_or("empty provider error")
            .to_string()
    } else {
        text.chars().take(500).collect()
    }
}

#[cfg(test)]
#[path = "../test/transport.rs"]
mod tests;
