package providers

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"sort"
	"strings"
	"sync"
	"time"

	aws "github.com/wagov-dtt/oy-cli/internal/oy/aws"
)

type ShimSpec struct {
	EnsureEnv   func(cwd string) error
	BuildClient func(cwd string) (CompletionClient, error)
	ListModels  func(cwd string) ([]string, error)
}

type ReasoningCache struct {
	mu    sync.Mutex
	items map[string]bool
}

var reasoningSupport = ReasoningCache{items: map[string]bool{}}

var ShimSpecs = map[string]ShimSpec{
	ShimOpenAI: {
		EnsureEnv: func(string) error {
			if strings.TrimSpace(os.Getenv("OPENAI_API_KEY")) == "" {
				return fmt.Errorf("OPENAI_API_KEY is not set")
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) { return OpenAIClientFromEnv(3) },
		ListModels:  func(string) ([]string, error) { return openAIListModelsFromEnv() },
	},
	ShimCodex: {
		EnsureEnv: func(string) error { _, err := LoadCodexSession(); return err },
		BuildClient: func(cwd string) (CompletionClient, error) {
			auth := LoadCodexAuth()
			apiKey, _ := auth["OPENAI_API_KEY"].(string)
			if strings.TrimSpace(apiKey) == "" {
				return nil, fmt.Errorf("Codex ChatGPT session client not implemented yet")
			}
			return OpenAIResponsesClient(apiKey, strings.TrimSpace(os.Getenv("OPENAI_BASE_URL")), loadCodexModelList, []string{"gpt-5"}, 3), nil
		},
		ListModels: func(string) ([]string, error) {
			items := loadCodexModelList()
			if len(items) == 0 {
				return []string{"gpt-5"}, nil
			}
			return items, nil
		},
	},
	ShimMantle: {
		EnsureEnv: func(cwd string) error {
			_, err := LoadAWSCredentials(cwd)
			return err
		},
		BuildClient: func(cwd string) (CompletionClient, error) {
			return MantleCompletionClient(cwd, "")
		},
		ListModels: func(cwd string) ([]string, error) {
			return LoadBedrockModelList(cwd, "")
		},
	},
	ShimCopilot: {
		EnsureEnv: func(string) error {
			if GetGitHubToken() == "" {
				return fmt.Errorf("No GitHub token found (set GH_TOKEN, GITHUB_TOKEN, or run `gh auth login`)")
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) {
			return nil, fmt.Errorf("copilot client not implemented yet")
		},
		ListModels: func(string) ([]string, error) { return nil, nil },
	},
	ShimOpenCode: {
		EnsureEnv: func(string) error {
			if OpencodeAPIKey("opencode") == "" {
				return fmt.Errorf("No OpenCode Zen credentials found in %s (run `opencode auth`)", OpencodeAuthPath)
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) {
			return ChatCompletionsClient(OpencodeAPIKey("opencode"), "https://api.opencode.ai/v1", nil, 3), nil
		},
		ListModels: func(string) ([]string, error) { return nil, nil },
	},
	ShimOpenCodeGo: {
		EnsureEnv: func(string) error {
			if OpencodeAPIKey("opencode-go") == "" {
				return fmt.Errorf("No OpenCode Go credentials found in %s (run `opencode auth`)", OpencodeAuthPath)
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) {
			return ChatCompletionsClient(OpencodeAPIKey("opencode-go"), "https://api.go.opencode.ai/v1", nil, 3), nil
		},
		ListModels: func(string) ([]string, error) { return nil, nil },
	},
}

func shimSpec(shim string) (ShimSpec, error) {
	if err := ValidateShim(shim); err != nil {
		return ShimSpec{}, err
	}
	spec, ok := ShimSpecs[shim]
	if !ok {
		return ShimSpec{}, fmt.Errorf("missing shim spec for %s", shim)
	}
	return spec, nil
}

func shimEnvError(spec ShimSpec, cwd string) string {
	if spec.EnsureEnv == nil {
		return ""
	}
	if err := spec.EnsureEnv(cwd); err != nil {
		return err.Error()
	}
	return ""
}

func shimAvailable(shim string) bool {
	spec, err := shimSpec(shim)
	if err != nil {
		return false
	}
	return shimEnvError(spec, "") == ""
}

func DetectAvailableShims() []string {
	items := make([]string, 0, len(ShimOrder))
	for _, shim := range ShimOrder {
		if shimAvailable(shim) {
			items = append(items, shim)
		}
	}
	return items
}

func ResolveShim(modelSpec, configuredShim string) string {
	if envShim := strings.TrimSpace(os.Getenv("OY_SHIM")); envShim != "" {
		return envShim
	}
	if prefix, _ := SplitModelSpec(modelSpec); prefix != "" {
		return prefix
	}
	if configuredShim != "" {
		return configuredShim
	}
	available := DetectAvailableShims()
	if len(available) > 0 {
		return available[0]
	}
	return ShimOpenAI
}

func EnsureAPIEnv(modelSpec, configuredShim, cwd string) (bool, string) {
	spec, err := shimSpec(ResolveShim(modelSpec, configuredShim))
	if err != nil {
		return false, err.Error()
	}
	if msg := shimEnvError(spec, cwd); msg != "" {
		return false, msg
	}
	return true, ""
}

func missingAPICredentialsMessage(errorText string) string {
	base := "Missing API credentials.\n\n- set `OPENAI_API_KEY`, or\n- sign in with Codex CLI (`codex login`), or\n- authenticate GitHub CLI for Copilot (`gh auth login`), or\n- authenticate with OpenCode (`opencode auth`), or\n- for Bedrock Mantle: configure AWS CLI credentials / SSO and set `AWS_REGION` (or `AWS_DEFAULT_REGION`) for the target region"
	if strings.TrimSpace(errorText) == "" {
		return base
	}
	return base + "\n- error: " + errorText
}

func RequireAPIEnv(modelSpec, configuredShim, cwd string) (string, error) {
	shim := ResolveShim(modelSpec, configuredShim)
	spec, err := shimSpec(shim)
	if err != nil {
		return "", err
	}
	if msg := shimEnvError(spec, cwd); msg != "" {
		return "", fmt.Errorf("%s", missingAPICredentialsMessage(msg))
	}
	return shim, nil
}

func GetClient(shim, cwd string) (CompletionClient, error) {
	spec, err := shimSpec(shim)
	if err != nil {
		return nil, err
	}
	if spec.BuildClient == nil {
		return nil, fmt.Errorf("shim %s does not implement a client", shim)
	}
	return spec.BuildClient(cwd)
}

func ListModelsForShim(shim, cwd string, ignoreErrors bool) ([]string, error) {
	spec, err := shimSpec(shim)
	if err != nil {
		return nil, err
	}
	if spec.ListModels == nil {
		return nil, nil
	}
	items, err := spec.ListModels(cwd)
	if err != nil {
		if ignoreErrors {
			return []string{}, nil
		}
		return nil, err
	}
	prefixed := make([]string, 0, len(items))
	for _, item := range items {
		prefixed = append(prefixed, JoinModelSpec(shim, item))
	}
	return prefixed, nil
}

func OpenAIClientFromEnv(maxRetries int) (CompletionClient, error) {
	apiKey := strings.TrimSpace(os.Getenv("OPENAI_API_KEY"))
	if apiKey == "" {
		return nil, fmt.Errorf("No OpenAI credentials found")
	}
	return OpenAIResponsesClient(apiKey, strings.TrimSpace(os.Getenv("OPENAI_BASE_URL")), nil, nil, maxRetries), nil
}

func loadCodexModelList() []string {
	auth := LoadCodexAuth()
	models := extractModelIDs(auth["models"], "id", "slug", "name")
	if len(models) == 0 {
		for _, key := range []string{"model", "default_model"} {
			if value, ok := auth[key].(string); ok && strings.TrimSpace(value) != "" {
				models = append(models, value)
			}
		}
	}
	if len(models) == 0 {
		return nil
	}
	seen := map[string]struct{}{}
	out := []string{}
	for _, item := range models {
		if _, ok := seen[item]; ok {
			continue
		}
		seen[item] = struct{}{}
		out = append(out, item)
	}
	sort.Strings(out)
	return out
}

func openAIListModelsFromEnv() ([]string, error) {
	apiKey := strings.TrimSpace(os.Getenv("OPENAI_API_KEY"))
	if apiKey == "" {
		return nil, fmt.Errorf("No OpenAI credentials found")
	}
	client := NewOpenAIHTTPClient(apiKey, strings.TrimSpace(os.Getenv("OPENAI_BASE_URL")), nil, ShortHTTPTimeout)
	return client.ListModelIDs()
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

func BedrockBaseURL(region string) string {
	return fmt.Sprintf("https://bedrock-mantle.%s.api.aws/v1", region)
}

var BedrockBaseURLFunc = BedrockBaseURL
var bedrockRequestHeadersFunc = BedrockRequestHeaders
var llmSessionFactory = func(timeout time.Duration, followRedirects bool) *HTTPClient {
	return LLMSession(timeout, followRedirects)
}

func BedrockRequestHeaders(credentials map[string]string, region, method, rawURL string, body []byte, headers map[string]string) (map[string]string, error) {
	cred := aws.Credentials{AccessKey: credentials["access_key"], SecretKey: credentials["secret_key"], SessionToken: credentials["session_token"]}
	return aws.SignV4Headers(cred, region, "bedrock-mantle", method, rawURL, body, headers, time.Time{})
}

func LoadBedrockModelList(cwd, region string) ([]string, error) {
	current := DefaultRegion(region)
	url := strings.TrimRight(BedrockBaseURLFunc(current), "/") + "/models"
	credentials, err := LoadAWSCredentials(cwd)
	if err != nil {
		return nil, err
	}
	headers, err := bedrockRequestHeadersFunc(credentials, current, http.MethodGet, url, nil, nil)
	if err != nil {
		return nil, err
	}
	response, err := llmSessionFactory(ShortHTTPTimeout, false).Request(http.MethodGet, url, headers, nil)
	if err != nil {
		return nil, err
	}
	if err := ResponseRaiseForStatus(response); err != nil {
		return nil, err
	}
	payload, err := responseJSONObject(response, "models: invalid JSON response")
	if err != nil {
		return nil, err
	}
	return extractModelIDs(payload["data"], "id"), nil
}

type mantleClient struct {
	credentials map[string]string
	region      string
	http        *HTTPClient
}

func (c *mantleClient) ChatCompletion(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error) {
	payload := map[string]any{
		"model":            model,
		"messages":         openAIChatMessages(messages),
		"reasoning_effort": "high",
	}
	if len(tools) > 0 {
		payload["tools"] = toolSpecsToOpenAI(tools)
		payload["tool_choice"] = toolChoice
	}
	result, err := callWithReasoningFallback("chat_completions", model, payload, func(payload map[string]any) (map[string]any, error) {
		body, err := encodeJSONBody(payload)
		if err != nil {
			return nil, err
		}
		url := strings.TrimRight(BedrockBaseURLFunc(c.region), "/") + "/chat/completions"
		headers, err := bedrockRequestHeadersFunc(c.credentials, c.region, http.MethodPost, url, body, map[string]string{"Content-Type": "application/json"})
		if err != nil {
			return nil, err
		}
		response, err := c.http.Request(http.MethodPost, url, headers, body)
		if err != nil {
			return nil, err
		}
		if err := ResponseRaiseForStatus(response); err != nil {
			return nil, err
		}
		return responseJSONObject(response, "Chat Completions API: invalid JSON response")
	})
	if err != nil {
		return ChatMessage{}, err
	}
	return chatCompletionToAssistantMessage(result)
}

func (c *mantleClient) ListModels() ([]string, error) { return LoadBedrockModelList("", c.region) }

func MantleCompletionClient(cwd, region string) (CompletionClient, error) {
	current := DefaultRegion(region)
	credentials, err := LoadAWSCredentials(cwd)
	if err != nil {
		return nil, err
	}
	return &mantleClient{credentials: credentials, region: current, http: llmSessionFactory(DefaultHTTPTimeout, false)}, nil
}

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

func LoadAWSCredentials(cwd string) (map[string]string, error) {
	result, err := awsCLI([]string{"configure", "export-credentials", "--format", "process", "--no-cli-pager"}, cwd, 30*time.Second)
	if err != nil {
		return nil, err
	}
	if result.ReturnCode != 0 {
		message := strings.TrimSpace(result.Stderr)
		if message == "" {
			message = strings.TrimSpace(result.Stdout)
		}
		if message == "" {
			message = fmt.Sprintf("AWS CLI exited with status %d", result.ReturnCode)
		}
		return nil, fmt.Errorf("%s", message)
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(result.Stdout), &payload); err != nil {
		return nil, fmt.Errorf("Could not parse AWS credentials JSON: %v", err)
	}
	accessKey, _ := payload["AccessKeyId"].(string)
	secretKey, _ := payload["SecretAccessKey"].(string)
	if accessKey == "" || secretKey == "" {
		return nil, fmt.Errorf("AWS CLI did not return AccessKeyId/SecretAccessKey")
	}
	out := map[string]string{"access_key": accessKey, "secret_key": secretKey}
	if token, _ := payload["SessionToken"].(string); token != "" {
		out["session_token"] = token
	}
	return out, nil
}

var awsCLIFunc = awsCLI

func awsCLI(parts []string, cwd string, timeout time.Duration) (CommandResult, error) {
	env := CommandEnv(cwd)
	awsPath := Which("aws", env["PATH"])
	if awsPath == "" {
		return CommandResult{}, fmt.Errorf("AWS CLI is not installed or not on PATH")
	}
	return RunCmd(append([]string{awsPath}, parts...), cwd, env, timeout, "")
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

var _ = bytes.MinRead
var _ = sort.Strings
var _ = url.Values{}
