package providers

import (
	"encoding/json"
	"strings"
)

func toolOutputValue(result ToolResult) any {
	content := NormalizeJSONLike(result.Content)
	if result.OK {
		return content
	}
	if data, ok := content.(map[string]any); ok {
		data["ok"] = false
		return data
	}
	return map[string]any{"ok": false, "message": content}
}

func toolOutputText(result ToolResult) string {
	text := SerializeJSON(toolOutputValue(result))
	text = strings.ReplaceAll(text, `"ok":false`, "ok: false")
	return text
}

func toolContentText(content string) string {
	var result ToolResult
	if err := json.Unmarshal([]byte(content), &result); err == nil {
		return toolOutputText(result)
	}
	return content
}

func openAIToolCall(toolCall ToolCall) map[string]any {
	return map[string]any{
		"id":   toolCall.ID,
		"type": "function",
		"function": map[string]any{
			"name":      toolCall.Name,
			"arguments": SerializeJSON(toolCall.Arguments),
		},
	}
}

func openAIChatMessage(message ChatMessage) map[string]any {
	switch message.Role {
	case "system", "user":
		return map[string]any{"role": message.Role, "content": message.Content}
	case "assistant":
		payload := map[string]any{"role": "assistant", "content": message.Content}
		if len(message.ToolCalls) > 0 {
			calls := make([]map[string]any, 0, len(message.ToolCalls))
			for _, toolCall := range message.ToolCalls {
				calls = append(calls, openAIToolCall(toolCall))
			}
			payload["tool_calls"] = calls
		}
		if len(message.ThoughtSignatures) > 0 {
			payload["thought_signatures"] = message.ThoughtSignatures
		}
		return payload
	case "tool":
		return map[string]any{"role": "tool", "content": toolContentText(message.Content), "tool_call_id": message.ToolCallID, "name": message.Name}
	default:
		panic("unsupported message role: " + message.Role)
	}
}

func openAIChatMessages(messages []ChatMessage) []map[string]any {
	items := make([]map[string]any, 0, len(messages))
	for _, message := range messages {
		items = append(items, openAIChatMessage(message))
	}
	return items
}

func responsesInputFromMessages(messages []ChatMessage) []map[string]any {
	items := []map[string]any{}
	for _, msg := range messages {
		switch msg.Role {
		case "system":
			continue
		case "user":
			items = append(items, map[string]any{"role": "user", "content": msg.Content})
		case "assistant":
			if strings.TrimSpace(msg.Content) != "" {
				items = append(items, map[string]any{"type": "message", "role": "assistant", "content": []map[string]any{{"type": "output_text", "text": msg.Content}}})
			}
			for _, call := range msg.ToolCalls {
				items = append(items, map[string]any{"type": "function_call", "call_id": call.ID, "name": call.Name, "arguments": SerializeJSON(call.Arguments)})
			}
		case "tool":
			items = append(items, map[string]any{"type": "function_call_output", "call_id": msg.ToolCallID, "output": toolContentText(msg.Content)})
		}
	}
	return items
}

func responsesPayload(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) map[string]any {
	payload := map[string]any{"model": model, "input": responsesInputFromMessages(messages), "store": false, "reasoning": map[string]any{"effort": "high"}}
	instructions := []string{}
	for _, message := range messages {
		if message.Role == "system" && strings.TrimSpace(message.Content) != "" {
			instructions = append(instructions, message.Content)
		}
	}
	if len(instructions) > 0 {
		payload["instructions"] = strings.Join(instructions, "\n\n")
	}
	if len(tools) > 0 {
		payload["tools"] = responsesTools(tools)
		payload["tool_choice"] = toolChoice
		payload["parallel_tool_calls"] = true
	}
	return payload
}

func responsesTools(tools []map[string]any) []map[string]any {
	result := make([]map[string]any, 0, len(tools))
	for _, tool := range tools {
		result = append(result, map[string]any{"type": "function", "name": tool["name"], "description": tool["description"], "parameters": defaultObjectSchema(tool["parameters"]), "strict": false})
	}
	return result
}

func toolSpecsToOpenAI(tools []map[string]any) []map[string]any {
	result := make([]map[string]any, 0, len(tools))
	for _, tool := range tools {
		result = append(result, map[string]any{"type": "function", "function": map[string]any{"name": tool["name"], "description": tool["description"], "parameters": defaultObjectSchema(tool["parameters"])}})
	}
	return result
}
