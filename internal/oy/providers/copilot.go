package providers

import (
	"sort"
	"strings"
)

func CopilotDefaultHeaders() map[string]string {
	return map[string]string{
		"Copilot-Integration-Id": copilotIntegrationID,
		"Editor-Version":         copilotEditorVersion,
	}
}

func FetchCopilotModelsRaw(token string) ([]map[string]any, error) {
	client := NewOpenAIHTTPClient(token, copilotBaseURL, CopilotDefaultHeaders(), ShortHTTPTimeout)
	response, err := client.request("GET", "/models", nil, nil)
	if err != nil {
		return nil, err
	}
	data, err := responseJSONObject(response, "Copilot models: invalid JSON response")
	if err != nil {
		return nil, err
	}
	rows, _ := data["data"].([]any)
	out := make([]map[string]any, 0, len(rows))
	for _, item := range rows {
		if row, ok := item.(map[string]any); ok {
			out = append(out, row)
		}
	}
	return out, nil
}

func ClassifyCopilotModels(token string) ([]string, map[string]struct{}, error) {
	raw, err := FetchCopilotModelsRaw(token)
	if err != nil {
		return nil, nil, err
	}
	chatIDs := []string{}
	responsesIDs := map[string]struct{}{}
	for _, model := range raw {
		modelID, _ := model["id"].(string)
		if strings.TrimSpace(modelID) == "" {
			continue
		}
		if caps, _ := model["capabilities"].(map[string]any); caps != nil {
			if kind, _ := caps["type"].(string); kind == "chat" {
				chatIDs = append(chatIDs, modelID)
			}
		}
		for _, endpoint := range asSlice(model["supported_endpoints"]) {
			if value, _ := endpoint.(string); value == "/responses" {
				responsesIDs[modelID] = struct{}{}
			}
		}
	}
	sort.Strings(chatIDs)
	return chatIDs, responsesIDs, nil
}

type funcClient struct {
	chatCompletion func(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error)
	listModels     func() ([]string, error)
}

func (c *funcClient) ChatCompletion(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error) {
	return c.chatCompletion(model, messages, tools, toolChoice)
}

func (c *funcClient) ListModels() ([]string, error) {
	return c.listModels()
}

func CopilotCompletionClient(token string) CompletionClient {
	client := NewOpenAIHTTPClient(token, copilotBaseURL, CopilotDefaultHeaders(), DefaultHTTPTimeout)
	client.MaxRetries = 0
	responsesModels := map[string]struct{}{}
	if _, supported, err := ClassifyCopilotModels(token); err == nil {
		responsesModels = supported
	}
	responsesInner := &responsesClient{
		create: func(payload map[string]any) (map[string]any, error) {
			return client.requestJSON("POST", "/responses", payload, nil)
		},
		list:     client.ListModelIDs,
		fallback: nil,
		defaults: nil,
	}
	chatInner := &chatClient{
		create: func(payload map[string]any) (map[string]any, error) {
			return client.requestJSON("POST", "/chat/completions", payload, nil)
		},
		list: client.ListModelIDs,
	}
	return &funcClient{
		chatCompletion: func(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error) {
			if _, ok := responsesModels[model]; ok {
				return responsesInner.ChatCompletion(model, messages, tools, toolChoice)
			}
			return chatInner.ChatCompletion(model, messages, tools, toolChoice)
		},
		listModels: func() ([]string, error) {
			if chatIDs, _, err := ClassifyCopilotModels(token); err == nil {
				return chatIDs, nil
			}
			return listModels(client.ListModelIDs, nil, []string{})
		},
	}
}
