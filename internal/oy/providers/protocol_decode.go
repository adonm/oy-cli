package providers

import (
	"encoding/json"
	"fmt"
	"strings"
)

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
