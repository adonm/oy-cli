package providers

import (
	"fmt"
	"os"
	"strings"
	"time"
)

func OpenAIClientFromEnv(maxRetries int) (CompletionClient, error) {
	apiKey := strings.TrimSpace(os.Getenv("OPENAI_API_KEY"))
	if apiKey == "" {
		return nil, fmt.Errorf("No OpenAI credentials found")
	}
	return OpenAIResponsesClient(apiKey, strings.TrimSpace(os.Getenv("OPENAI_BASE_URL")), nil, nil, maxRetries), nil
}

func openAIListModelsFromEnv() ([]string, error) {
	apiKey := strings.TrimSpace(os.Getenv("OPENAI_API_KEY"))
	if apiKey == "" {
		return nil, fmt.Errorf("No OpenAI credentials found")
	}
	client := NewOpenAIHTTPClient(apiKey, strings.TrimSpace(os.Getenv("OPENAI_BASE_URL")), nil, ShortHTTPTimeout)
	return client.ListModelIDs()
}

func mustJSONBody(value any) []byte {
	data, err := encodeJSONBody(value)
	if err != nil {
		panic(err)
	}
	return data
}

type OpenAIHTTPClient struct {
	APIKey     string
	BaseURL    string
	Headers    map[string]string
	HTTP       *HTTPClient
	MaxRetries int
}

func NewOpenAIHTTPClient(apiKey, baseURL string, headers map[string]string, timeout time.Duration) *OpenAIHTTPClient {
	if strings.TrimSpace(baseURL) == "" {
		baseURL = "https://api.openai.com/v1"
	}
	copied := map[string]string{}
	for key, value := range headers {
		copied[key] = value
	}
	return &OpenAIHTTPClient{APIKey: apiKey, BaseURL: strings.TrimRight(baseURL, "/"), Headers: copied, HTTP: LLMSession(timeout, false), MaxRetries: 3}
}

func (c *OpenAIHTTPClient) requestJSON(method, path string, payload any, extra map[string]string) (map[string]any, error) {
	body, err := encodeJSONBody(payload)
	if err != nil {
		return nil, err
	}
	response, err := c.request(method, path, body, extra)
	if err != nil {
		return nil, err
	}
	return responseJSONObject(response, path+": invalid JSON response")
}

func (c *OpenAIHTTPClient) request(method, path string, body []byte, extra map[string]string) (ResponseAdapter, error) {
	headers := map[string]string{"Authorization": "Bearer " + c.APIKey}
	for key, value := range c.Headers {
		headers[key] = value
	}
	for key, value := range extra {
		headers[key] = value
	}
	if len(body) > 0 {
		headers["Content-Type"] = "application/json"
	}
	response, err := c.HTTP.Request(method, c.BaseURL+path, headers, body)
	if err != nil {
		return ResponseAdapter{}, err
	}
	if err := ResponseRaiseForStatus(response); err != nil {
		return ResponseAdapter{}, err
	}
	return response, nil
}

func (c *OpenAIHTTPClient) ListModelIDs() ([]string, error) {
	response, err := c.request("GET", "/models", nil, nil)
	if err != nil {
		return nil, err
	}
	data, err := responseJSONObject(response, "models: invalid JSON response")
	if err != nil {
		return nil, err
	}
	return extractModelIDs(data["data"], "id"), nil
}

type responsesClient struct {
	create   func(payload map[string]any) (map[string]any, error)
	list     func() ([]string, error)
	fallback func() []string
	defaults []string
}

func (c *responsesClient) ChatCompletion(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error) {
	payload := responsesPayload(model, messages, tools, toolChoice)
	result, err := callWithReasoningFallback("responses", model, payload, c.create)
	if err != nil {
		return ChatMessage{}, err
	}
	return decodeResponsesOutput(result)
}

func (c *responsesClient) ListModels() ([]string, error) {
	return listModels(c.list, c.fallback, c.defaults)
}

type chatClient struct {
	create func(payload map[string]any) (map[string]any, error)
	list   func() ([]string, error)
}

func (c *chatClient) ChatCompletion(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error) {
	payload := map[string]any{
		"model":            model,
		"messages":         openAIChatMessages(messages),
		"reasoning_effort": "high",
	}
	if len(tools) > 0 {
		payload["tools"] = toolSpecsToOpenAI(tools)
		payload["tool_choice"] = toolChoice
	}
	result, err := callWithReasoningFallback("chat_completions", model, payload, c.create)
	if err != nil {
		return ChatMessage{}, err
	}
	return chatCompletionToAssistantMessage(result)
}

func (c *chatClient) ListModels() ([]string, error) { return listModels(c.list, nil, nil) }

func OpenAIResponsesClient(apiKey, baseURL string, fallback func() []string, defaults []string, maxRetries int) CompletionClient {
	httpClient := NewOpenAIHTTPClient(apiKey, baseURL, nil, DefaultHTTPTimeout)
	httpClient.MaxRetries = maxRetries
	return &responsesClient{
		create: func(payload map[string]any) (map[string]any, error) {
			return httpClient.requestJSON("POST", "/responses", payload, nil)
		},
		list:     httpClient.ListModelIDs,
		fallback: fallback,
		defaults: defaults,
	}
}

func ChatCompletionsClient(apiKey, baseURL string, headers map[string]string, maxRetries int) CompletionClient {
	httpClient := NewOpenAIHTTPClient(apiKey, baseURL, headers, DefaultHTTPTimeout)
	httpClient.MaxRetries = maxRetries
	return &chatClient{
		create: func(payload map[string]any) (map[string]any, error) {
			return httpClient.requestJSON("POST", "/chat/completions", payload, nil)
		},
		list: httpClient.ListModelIDs,
	}
}
