package providers

import (
	"encoding/json"
	"fmt"
	"strings"
	"sync"
)

type ReasoningCache struct {
	mu    sync.Mutex
	items map[string]bool
}

var reasoningSupport = ReasoningCache{items: map[string]bool{}}

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

func decodeToolCallArguments(arguments any) (map[string]any, error) {
	switch value := arguments.(type) {
	case nil:
		return map[string]any{}, nil
	case map[string]any:
		return value, nil
	case string:
		if strings.TrimSpace(value) == "" {
			return map[string]any{}, nil
		}
		decode := func(candidate string) (map[string]any, error) {
			var parsed any
			if err := json.Unmarshal([]byte(candidate), &parsed); err != nil {
				return nil, err
			}
			if nested, ok := parsed.(string); ok {
				if err := json.Unmarshal([]byte(nested), &parsed); err != nil {
					return nil, err
				}
			}
			data, ok := parsed.(map[string]any)
			if !ok {
				return nil, fmt.Errorf("tool arguments must decode to a JSON object")
			}
			return data, nil
		}
		if data, err := decode(value); err == nil {
			return data, nil
		}
		mid := len(value) / 2
		for i := max(0, mid-40); i < min(len(value), mid+40); i++ {
			if value[i] != '{' {
				continue
			}
			if data, err := decode(value[i:]); err == nil {
				return data, nil
			}
		}
		return nil, fmt.Errorf("Could not parse tool arguments JSON")
	default:
		return nil, fmt.Errorf("tool arguments must be a JSON object or JSON string")
	}
}

func decodeResponsesOutput(response map[string]any) (ChatMessage, error) {
	items, _ := response["output"].([]any)
	contentParts := []string{}
	var toolCalls []ToolCall
	for _, item := range items {
		data, _ := item.(map[string]any)
		typeName, _ := data["type"].(string)
		switch typeName {
		case "message":
			if role, _ := data["role"].(string); role != "assistant" {
				continue
			}
			parts, _ := data["content"].([]any)
			for _, part := range parts {
				entry, _ := part.(map[string]any)
				if text, _ := entry["text"].(string); strings.TrimSpace(text) != "" {
					contentParts = append(contentParts, text)
				} else if refusal, _ := entry["refusal"].(string); strings.TrimSpace(refusal) != "" {
					contentParts = append(contentParts, refusal)
				}
			}
		case "function_call":
			callID, _ := firstNonEmptyString(data, "call_id", "id")
			name, _ := data["name"].(string)
			arguments, err := decodeToolCallArguments(data["arguments"])
			if err != nil || callID == "" || name == "" {
				continue
			}
			toolCalls = append(toolCalls, ToolCallMessage(callID, name, arguments))
		}
	}
	if len(contentParts) == 0 {
		if outputText, _ := response["output_text"].(string); strings.TrimSpace(outputText) != "" {
			contentParts = append(contentParts, outputText)
		}
	}
	return AssistantMessage(strings.Join(contentParts, "\n\n"), toolCalls), nil
}

func chatCompletionToAssistantMessage(response map[string]any) (ChatMessage, error) {
	choices, _ := response["choices"].([]any)
	if len(choices) == 0 {
		return AssistantMessage("", nil), nil
	}
	messageData := map[string]any{}
	for _, choice := range choices {
		item, _ := choice.(map[string]any)
		message, _ := item["message"].(map[string]any)
		for key, value := range message {
			if current, ok := messageData[key]; !ok || isBlankChatValue(current) {
				messageData[key] = value
			}
		}
	}
	content, _ := messageData["content"].(string)
	if content == "" {
		if reasoning, _ := messageData["reasoning"].(string); reasoning != "" {
			content = reasoning
		}
	}
	var toolCalls []ToolCall
	for _, item := range asSlice(messageData["tool_calls"]) {
		data, _ := item.(map[string]any)
		callID, _ := data["id"].(string)
		function, _ := data["function"].(map[string]any)
		name, _ := function["name"].(string)
		arguments, err := decodeToolCallArguments(function["arguments"])
		if err != nil || callID == "" || name == "" {
			continue
		}
		toolCalls = append(toolCalls, ToolCallMessage(callID, name, arguments))
	}
	return AssistantMessage(content, toolCalls), nil
}

func callWithReasoningFallback(apiKind, model string, payload map[string]any, create func(map[string]any) (map[string]any, error)) (map[string]any, error) {
	if !shouldSendReasoning(apiKind, model) {
		payload = dropReasoningArg(payload)
	}
	result, err := create(payload)
	if err == nil {
		return result, nil
	}
	statusErr := &APIStatusError{}
	if !AsAPIStatusError(err, &statusErr) || !isReasoningUnsupportedError(statusErr) {
		return nil, err
	}
	markReasoningUnsupported(apiKind, model)
	return create(dropReasoningArg(payload))
}

func shouldSendReasoning(apiKind, model string) bool {
	reasoningSupport.mu.Lock()
	defer reasoningSupport.mu.Unlock()
	value, ok := reasoningSupport.items[apiKind+"\x00"+model]
	if !ok {
		return true
	}
	return value
}

func markReasoningUnsupported(apiKind, model string) {
	reasoningSupport.mu.Lock()
	defer reasoningSupport.mu.Unlock()
	reasoningSupport.items[apiKind+"\x00"+model] = false
}

func isReasoningUnsupportedError(err *APIStatusError) bool {
	if err.Response.StatusCode != 400 {
		return false
	}
	message := strings.ToLower(ResponseErrorMessage(err.Response))
	return strings.Contains(message, "reasoning") && (strings.Contains(message, "unsupported") || strings.Contains(message, "unknown parameter") || strings.Contains(message, "not allowed") || strings.Contains(message, "not supported") || strings.Contains(message, "invalid parameter") || strings.Contains(message, "extra inputs"))
}

func dropReasoningArg(payload map[string]any) map[string]any {
	out := map[string]any{}
	for key, value := range payload {
		if key == "reasoning" || key == "reasoning_effort" {
			continue
		}
		out[key] = value
	}
	return out
}

func listModels(list func() ([]string, error), fallback func() []string, defaults []string) ([]string, error) {
	items, err := list()
	if err == nil {
		return items, nil
	}
	if fallback != nil {
		if cached := fallback(); len(cached) > 0 {
			return cached, nil
		}
	}
	if defaults != nil {
		return defaults, nil
	}
	return nil, err
}

func defaultObjectSchema(value any) map[string]any {
	if data, ok := value.(map[string]any); ok {
		return data
	}
	return map[string]any{"type": "object"}
}

func encodeJSONBody(value any) ([]byte, error) {
	return json.Marshal(value)
}

func responseJSONObject(response ResponseAdapter, errorText string) (map[string]any, error) {
	payload, err := ResponseJSON(response)
	if err != nil {
		return nil, fmt.Errorf("%s", errorText)
	}
	data, ok := payload.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("%s", errorText)
	}
	return data, nil
}

func extractModelIDs(items any, keys ...string) []string {
	rows, ok := items.([]any)
	if !ok {
		return []string{}
	}
	seen := map[string]struct{}{}
	out := []string{}
	for _, item := range rows {
		data, ok := item.(map[string]any)
		if !ok {
			continue
		}
		if value, ok := firstNonEmptyString(data, keys...); ok {
			if _, done := seen[value]; !done {
				seen[value] = struct{}{}
				out = append(out, value)
			}
		}
	}
	return out
}

func firstNonEmptyString(data map[string]any, keys ...string) (string, bool) {
	for _, key := range keys {
		if value, ok := data[key].(string); ok && strings.TrimSpace(value) != "" {
			return value, true
		}
	}
	return "", false
}

func isBlankChatValue(value any) bool {
	switch item := value.(type) {
	case nil:
		return true
	case string:
		return strings.TrimSpace(item) == ""
	case []any:
		return len(item) == 0
	case map[string]any:
		return len(item) == 0
	default:
		return false
	}
}

func asSlice(value any) []any {
	items, _ := value.([]any)
	return items
}

func AsAPIStatusError(err error, target **APIStatusError) bool {
	if err == nil {
		return false
	}
	if value, ok := err.(*APIStatusError); ok {
		*target = value
		return true
	}
	switch value := err.(type) {
	case *AuthenticationError:
		base := value.APIStatusError
		*target = &base
		return true
	case *PermissionDeniedError:
		base := value.APIStatusError
		*target = &base
		return true
	case *RateLimitError:
		base := value.APIStatusError
		*target = &base
		return true
	case *BadRequestError:
		base := value.APIStatusError
		*target = &base
		return true
	default:
		return false
	}
}
