use anyhow::{Result, bail};
use std::collections::HashMap;
use std::hash::Hash;

use crate::llm::schema::{LlmEvent, ToolCall};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PendingTool {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) input: String,
}

pub(crate) type State<K> = HashMap<K, PendingTool>;

pub(crate) fn start<K>(tools: &mut State<K>, key: K, tool: PendingTool)
where
    K: Eq + Hash,
{
    tools.insert(key, tool);
}

pub(crate) fn append_or_start<K>(
    tools: &mut State<K>,
    key: K,
    id: Option<&str>,
    name: Option<&str>,
    text: Option<&str>,
    missing_tool_message: &str,
) -> Result<Vec<LlmEvent>>
where
    K: Eq + Hash,
{
    let current = tools.get(&key);
    let id = id.or_else(|| current.map(|tool| tool.id.as_str()));
    let name = name.or_else(|| current.map(|tool| tool.name.as_str()));
    let (Some(id), Some(name)) = (id, name) else {
        bail!("{missing_tool_message}");
    };
    let text = text.unwrap_or_default();
    let mut tool = current.cloned().unwrap_or_else(|| PendingTool {
        id: id.to_string(),
        name: name.to_string(),
        input: String::new(),
    });
    tool.id = id.to_string();
    tool.name = name.to_string();
    tool.input.push_str(text);
    let is_new = !tools.contains_key(&key);
    tools.insert(key, tool.clone());

    let mut events = Vec::new();
    if is_new {
        events.push(LlmEvent::ToolInputStart {
            id: tool.id.clone(),
            name: tool.name.clone(),
        });
    }
    if !text.is_empty() {
        events.push(LlmEvent::ToolInputDelta {
            text: text.to_string(),
        });
    }
    Ok(events)
}

pub(crate) fn append_existing<K>(
    tools: &mut State<K>,
    key: &K,
    text: &str,
    missing_tool_message: &str,
) -> Result<Vec<LlmEvent>>
where
    K: Eq + Hash,
{
    let Some(tool) = tools.get_mut(key) else {
        bail!("{missing_tool_message}");
    };
    if text.is_empty() {
        return Ok(Vec::new());
    }
    tool.input.push_str(text);
    Ok(vec![LlmEvent::ToolInputDelta {
        text: text.to_string(),
    }])
}

pub(crate) fn finish<K>(route: &str, tools: &mut State<K>, key: &K) -> Result<Vec<LlmEvent>>
where
    K: Eq + Hash,
{
    let Some(tool) = tools.remove(key) else {
        return Ok(Vec::new());
    };
    finish_events(route, tool, None)
}

pub(crate) fn finish_with_input<K>(
    route: &str,
    tools: &mut State<K>,
    key: &K,
    input: &str,
) -> Result<Vec<LlmEvent>>
where
    K: Eq + Hash,
{
    let Some(tool) = tools.remove(key) else {
        return Ok(Vec::new());
    };
    finish_events(route, tool, Some(input))
}

pub(crate) fn finish_all<K>(route: &str, tools: &mut State<K>) -> Result<Vec<LlmEvent>>
where
    K: Clone + Eq + Hash + Ord,
{
    let mut keys = tools.keys().cloned().collect::<Vec<_>>();
    keys.sort_unstable();
    let mut events = Vec::new();
    for key in keys {
        events.extend(finish(route, tools, &key)?);
    }
    Ok(events)
}

fn finish_events(
    route: &str,
    tool: PendingTool,
    input_override: Option<&str>,
) -> Result<Vec<LlmEvent>> {
    let input = input_override.unwrap_or(&tool.input);
    let call = ToolCall::from_raw_input(tool.id.clone(), tool.name.clone(), input, route)?;
    Ok(vec![
        LlmEvent::ToolInputEnd {
            id: tool.id,
            name: tool.name,
        },
        LlmEvent::ToolCall {
            call,
            provider_executed: false,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn append_or_start_requires_identity_on_first_delta() {
        let mut tools = State::<usize>::new();

        let err =
            append_or_start(&mut tools, 0, None, None, Some("{}"), "missing identity").unwrap_err();

        assert_eq!(err.to_string(), "missing identity");
    }

    #[test]
    fn finish_all_parses_inputs_in_key_order() {
        let mut tools = State::new();
        append_or_start(
            &mut tools,
            1,
            Some("call-b"),
            Some("read"),
            Some("{\"path\":"),
            "missing",
        )
        .unwrap();
        append_or_start(
            &mut tools,
            0,
            Some("call-a"),
            Some("read"),
            Some("{\"path\":"),
            "missing",
        )
        .unwrap();
        append_or_start(&mut tools, 1, None, None, Some("\"B\"}"), "missing").unwrap();
        append_or_start(&mut tools, 0, None, None, Some("\"A\"}"), "missing").unwrap();

        let events = finish_all("test-route", &mut tools).unwrap();
        let calls = events
            .into_iter()
            .filter_map(|event| match event {
                LlmEvent::ToolCall { call, .. } => Some(call),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(calls[0].call_id, "call-a");
        assert_eq!(calls[0].arguments_value().unwrap(), json!({"path": "A"}));
        assert_eq!(calls[1].call_id, "call-b");
        assert_eq!(tools.len(), 0);
    }
}
