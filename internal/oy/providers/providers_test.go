package providers

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

type stubClient struct {
	models []string
}

func (s stubClient) ChatCompletion(string, []ChatMessage, []map[string]any, string) (ChatMessage, error) {
	return ChatMessage{}, nil
}
func (s stubClient) ListModels() ([]string, error) { return s.models, nil }

func TestSplitJoinModelSpec(t *testing.T) {
	shim, model := SplitModelSpec("openai:gpt-test")
	if shim != "openai" || model != "gpt-test" {
		t.Fatalf("unexpected split: %q %q", shim, model)
	}
	if got := JoinModelSpec("copilot", "gpt-5"); got != "copilot:gpt-5" {
		t.Fatalf("unexpected join: %q", got)
	}
}

func TestAuthLoadersIgnoreNonObjectJSON(t *testing.T) {
	tmp := t.TempDir()
	codex := filepath.Join(tmp, "codex.json")
	opencode := filepath.Join(tmp, "opencode.json")
	if err := os.WriteFile(codex, []byte("[]"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(opencode, []byte("[]"), 0o600); err != nil {
		t.Fatal(err)
	}
	oldCodex, oldOpenCode := CodexAuthPath, OpencodeAuthPath
	CodexAuthPath, OpencodeAuthPath = codex, opencode
	defer func() {
		CodexAuthPath, OpencodeAuthPath = oldCodex, oldOpenCode
	}()
	if got := LoadCodexAuth(); !reflect.DeepEqual(got, map[string]any{}) {
		t.Fatalf("unexpected codex auth: %#v", got)
	}
	if got := OpencodeAPIKey("opencode"); got != "" {
		t.Fatalf("unexpected opencode key: %q", got)
	}
	if got := OpencodeAPIKey("opencode-go"); got != "" {
		t.Fatalf("unexpected opencode-go key: %q", got)
	}
}

func TestResponseRaiseForStatus(t *testing.T) {
	response := ResponseAdapter{StatusCode: 401, Text: `{"error":{"message":"bad token"}}`, ReasonPhrase: "Unauthorized"}
	err := ResponseRaiseForStatus(response)
	if _, ok := err.(*AuthenticationError); !ok {
		t.Fatalf("unexpected error type: %T", err)
	}
	if err.Error() != "bad token" {
		t.Fatalf("unexpected error message: %v", err)
	}
}

func TestBedrockMantleRequestHeaders(t *testing.T) {
	headers, err := BedrockRequestHeaders(
		map[string]string{"access_key": "AKIDEXAMPLE", "secret_key": "wJalrXUtnFEMI/I/K7MDENG+bPxRfiCYEXAMPLEKEY", "session_token": "TOKEN"},
		"ap-southeast-2",
		"POST",
		"https://bedrock-mantle.ap-southeast-2.api.aws/v1/chat/completions",
		[]byte(`{"model":"zai.glm-4.6"}`),
		map[string]string{"Content-Type": "application/json"},
	)
	if err != nil {
		t.Fatal(err)
	}
	if headers["Content-Type"] != "application/json" || headers["Host"] != "bedrock-mantle.ap-southeast-2.api.aws" || headers["X-Amz-Security-Token"] != "TOKEN" || !strings.Contains(headers["Authorization"], "Credential=AKIDEXAMPLE/") || !strings.Contains(headers["Authorization"], "/ap-southeast-2/bedrock-mantle/aws4_request") {
		t.Fatalf("unexpected headers: %#v", headers)
	}
}

func TestDecodeToolCallArgumentsAndOutputs(t *testing.T) {
	encoded, err := json.MarshalIndent(map[string]any{"count": 2}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := decodeToolCallArguments(encoded)
	if err == nil || decoded != nil {
		t.Fatalf("expected non-string decode failure, got %#v %v", decoded, err)
	}
	parsed, err := decodeToolCallArguments(`"{\"count\":2}"`)
	if err != nil || !reflect.DeepEqual(parsed, map[string]any{"count": float64(2)}) {
		t.Fatalf("unexpected parsed args: %#v %v", parsed, err)
	}
	parsed, err = decodeToolCallArguments(`{"ok":true}{"ok":true}`)
	if err != nil || !reflect.DeepEqual(parsed, map[string]any{"ok": true}) {
		t.Fatalf("unexpected duplicated parsed args: %#v %v", parsed, err)
	}
	result := NewToolResult(false, map[string]any{"tool": "read", "error_type": "ValueError", "message": "read path does not exist: missing.txt"})
	value := toolOutputValue(result)
	if !reflect.DeepEqual(value, map[string]any{"ok": false, "tool": "read", "error_type": "ValueError", "message": "read path does not exist: missing.txt"}) {
		t.Fatalf("unexpected tool output value: %#v", value)
	}
	openAITool := openAIChatMessage(ToolMessage("call_1", "read", result))
	if !strings.Contains(openAITool["content"].(string), "ok: false") || !strings.Contains(openAITool["content"].(string), "read path does not exist: missing.txt") {
		t.Fatalf("unexpected tool message payload: %#v", openAITool)
	}
	responsesInput := responsesInputFromMessages([]ChatMessage{ToolMessage("call_1", "read", result)})
	if len(responsesInput) != 1 || responsesInput[0]["type"].(string) != "function_call_output" || !strings.Contains(responsesInput[0]["output"].(string), "ok: false") {
		t.Fatalf("unexpected responses input: %#v", responsesInput)
	}
}

func TestResponsesAndChatDecoding(t *testing.T) {
	decoded, err := decodeResponsesOutput(map[string]any{
		"output": []any{
			map[string]any{"type": "message", "role": "assistant", "content": []any{map[string]any{"text": "hello"}, map[string]any{"refusal": "nope"}, map[string]any{"text": "   "}}},
			map[string]any{"type": "function_call", "call_id": "call_1", "name": "echo", "arguments": `{"value":"x"}`},
		},
	})
	if err != nil || !reflect.DeepEqual(decoded, AssistantMessage("hello\n\nnope", []ToolCall{ToolCallMessage("call_1", "echo", map[string]any{"value": "x"})})) {
		t.Fatalf("unexpected responses decoding: %#v %v", decoded, err)
	}
	chatMessage, err := chatCompletionToAssistantMessage(map[string]any{"choices": []any{
		map[string]any{"message": map[string]any{"content": "hello", "tool_calls": nil}},
		map[string]any{"message": map[string]any{"content": "hello", "tool_calls": []any{map[string]any{"id": "call_2", "function": map[string]any{"name": "echo", "arguments": `{"count":2}`}}}}},
	}})
	if err != nil || !reflect.DeepEqual(chatMessage, AssistantMessage("hello", []ToolCall{ToolCallMessage("call_2", "echo", map[string]any{"count": float64(2)})})) {
		t.Fatalf("unexpected chat decoding: %#v %v", chatMessage, err)
	}
	reasoningOnly, err := chatCompletionToAssistantMessage(map[string]any{"choices": []any{map[string]any{"message": map[string]any{"content": "", "reasoning": "thoughts"}}}})
	if err != nil || !reflect.DeepEqual(reasoningOnly, AssistantMessage("thoughts", nil)) {
		t.Fatalf("unexpected reasoning-only decode: %#v %v", reasoningOnly, err)
	}
}

func TestReasoningFallback(t *testing.T) {
	reasoningSupport = ReasoningCache{items: map[string]bool{}}
	calls := []map[string]any{}
	_, err := callWithReasoningFallback("responses", "gpt-test", map[string]any{"reasoning": map[string]any{"effort": "high"}}, func(payload map[string]any) (map[string]any, error) {
		calls = append(calls, payload)
		if len(calls) == 1 {
			return nil, &BadRequestError{APIStatusError: APIStatusError{Message: "Unsupported parameter: reasoning", Response: ResponseAdapter{StatusCode: 400, Text: `{"error":{"message":"Unsupported parameter: reasoning"}}`}}}
		}
		return map[string]any{"output": []any{}}, nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if calls[0]["reasoning"].(map[string]any)["effort"].(string) != "high" {
		t.Fatalf("expected reasoning on first call: %#v", calls)
	}
	if _, ok := calls[1]["reasoning"]; ok {
		t.Fatalf("expected reasoning to be dropped on retry: %#v", calls)
	}
	cachedCalls := []map[string]any{}
	_, err = callWithReasoningFallback("responses", "gpt-test", map[string]any{"reasoning": map[string]any{"effort": "high"}}, func(payload map[string]any) (map[string]any, error) {
		cachedCalls = append(cachedCalls, payload)
		return map[string]any{"output": []any{}}, nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := cachedCalls[0]["reasoning"]; ok {
		t.Fatalf("expected cached reasoning drop: %#v", cachedCalls)
	}
}

func TestMantleAndOpenAIClients(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/models":
			_, _ = w.Write([]byte(`{"data":[{"id":"gpt-test"}]}`))
		case "/v1/responses":
			_, _ = w.Write([]byte(`{"output":[{"type":"message","role":"assistant","content":[{"text":"done"}]}]}`))
		case "/v1/chat/completions":
			_, _ = w.Write([]byte(`{"choices":[{"message":{"content":"done"}}]}`))
		default:
			w.WriteHeader(404)
		}
	}))
	defer server.Close()
	responses := OpenAIResponsesClient("key", server.URL+"/v1", nil, nil, 3)
	message, err := responses.ChatCompletion("gpt-test", nil, nil, "auto")
	if err != nil || message.Content != "done" {
		t.Fatalf("unexpected responses client result: %#v %v", message, err)
	}
	models, err := responses.ListModels()
	if err != nil || !reflect.DeepEqual(models, []string{"gpt-test"}) {
		t.Fatalf("unexpected responses models: %#v %v", models, err)
	}
	chat := ChatCompletionsClient("key", server.URL+"/v1", nil, 3)
	message, err = chat.ChatCompletion("gpt-test", nil, nil, "auto")
	if err != nil || message.Content != "done" {
		t.Fatalf("unexpected chat client result: %#v %v", message, err)
	}
}

func TestLoadBedrockModelListUsesMantleEndpoint(t *testing.T) {
	requested := map[string]any{}
	oldHeaders, oldSession, oldAWS := bedrockRequestHeadersFunc, llmSessionFactory, awsCLIFunc
	defer func() { bedrockRequestHeadersFunc, llmSessionFactory, awsCLIFunc = oldHeaders, oldSession, oldAWS }()
	awsCLIFunc = func(parts []string, cwd string, timeout time.Duration) (CommandResult, error) {
		_ = parts
		_ = cwd
		_ = timeout
		return CommandResult{Stdout: `{"AccessKeyId":"AKIDEXAMPLE","SecretAccessKey":"SECRET","SessionToken":"TOKEN"}`}, nil
	}
	bedrockRequestHeadersFunc = func(credentials map[string]string, region, method, url string, body []byte, headers map[string]string) (map[string]string, error) {
		_ = credentials
		_ = region
		_ = method
		_ = url
		_ = body
		return map[string]string{"Authorization": "AWS4-HMAC-SHA256 test", "X-Amz-Date": "20260327T062009Z", "X-Amz-Security-Token": "TOKEN"}, nil
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requested["method"] = r.Method
		requested["url"] = r.URL.String()
		requested["authorization"] = r.Header.Get("Authorization")
		_, _ = w.Write([]byte(`{"data":[{"id":"zai.glm-4.6"},{"id":"moonshotai.kimi-k2-thinking"}]}`))
	}))
	defer server.Close()
	baseURL := server.URL
	oldBedrockBaseURL := BedrockBaseURLFunc
	defer func() { BedrockBaseURLFunc = oldBedrockBaseURL }()
	BedrockBaseURLFunc = func(region string) string {
		_ = region
		return baseURL
	}
	llmSessionFactory = func(timeout time.Duration, followRedirects bool) *HTTPClient {
		_ = timeout
		_ = followRedirects
		client := NewHTTPClient(ShortHTTPTimeout, false)
		client.client = server.Client()
		return &HTTPClient{Timeout: client.Timeout, FollowRedirects: client.FollowRedirects, client: client.client}
	}
	models, err := LoadBedrockModelList(t.TempDir(), "ap-southeast-2")
	if err != nil || !reflect.DeepEqual(models, []string{"zai.glm-4.6", "moonshotai.kimi-k2-thinking"}) {
		t.Fatalf("unexpected bedrock model list: %#v %v", models, err)
	}
	if requested["method"] != "GET" || requested["authorization"] != "AWS4-HMAC-SHA256 test" || !strings.Contains(requested["url"].(string), "/models") {
		t.Fatalf("unexpected request capture: %#v", requested)
	}
}

func TestShimRegistryOrderAndClientBuilding(t *testing.T) {
	t.Setenv("OY_SHIM", "")
	calls := []string{}
	sentinel := stubClient{models: []string{"demo"}}
	oldOrder, oldKnown, oldSpecs := ShimOrder, KnownShims, ShimSpecs
	defer func() {
		ShimOrder, KnownShims, ShimSpecs = oldOrder, oldKnown, oldSpecs
	}()
	okEnsure := func(name string) func(string) error {
		return func(cwd string) error {
			calls = append(calls, name+":"+cwd)
			return nil
		}
	}
	failEnsure := func(name string) func(string) error {
		return func(cwd string) error {
			calls = append(calls, name+":"+cwd)
			return assertErr(name)
		}
	}
	ShimOrder = []string{"alpha", "beta", "gamma"}
	KnownShims = map[string]struct{}{"alpha": {}, "beta": {}, "gamma": {}}
	ShimSpecs = map[string]ShimSpec{
		"alpha": {EnsureEnv: okEnsure("alpha"), BuildClient: func(string) (CompletionClient, error) { return sentinel, nil }, ListModels: func(string) ([]string, error) { return []string{"demo"}, nil }},
		"beta":  {EnsureEnv: failEnsure("beta"), BuildClient: func(string) (CompletionClient, error) { return nil, nil }, ListModels: func(string) ([]string, error) { return nil, nil }},
		"gamma": {EnsureEnv: okEnsure("gamma"), BuildClient: func(string) (CompletionClient, error) { return nil, nil }, ListModels: func(string) ([]string, error) { return []string{}, nil }},
	}
	available := DetectAvailableShims()
	if !reflect.DeepEqual(available, []string{"alpha", "gamma"}) {
		t.Fatalf("unexpected available shims: %#v", available)
	}
	if !reflect.DeepEqual(calls, []string{"alpha:", "beta:", "gamma:"}) {
		t.Fatalf("unexpected ensure calls: %#v", calls)
	}
	ok, msg := EnsureAPIEnv("alpha:model", "", "/tmp/work")
	if !ok || msg != "" {
		t.Fatalf("unexpected ensure result: %v %q", ok, msg)
	}
	client, err := GetClient("alpha", "/tmp/work")
	if err != nil {
		t.Fatal(err)
	}
	models, err := client.ListModels()
	if err != nil || !reflect.DeepEqual(models, []string{"demo"}) {
		t.Fatalf("unexpected models: %#v %v", models, err)
	}
	prefixed, err := ListModelsForShim("alpha", "/tmp/work", true)
	if err != nil || !reflect.DeepEqual(prefixed, []string{"alpha:demo"}) {
		t.Fatalf("unexpected prefixed models: %#v %v", prefixed, err)
	}
}

func TestListModelsErrorHandling(t *testing.T) {
	t.Setenv("OY_SHIM", "")
	oldOrder, oldKnown, oldSpecs := ShimOrder, KnownShims, ShimSpecs
	defer func() {
		ShimOrder, KnownShims, ShimSpecs = oldOrder, oldKnown, oldSpecs
	}()
	ShimOrder = []string{"gamma"}
	KnownShims = map[string]struct{}{"gamma": {}}
	ShimSpecs = map[string]ShimSpec{
		"gamma": {EnsureEnv: func(string) error { return nil }, BuildClient: func(string) (CompletionClient, error) { return nil, nil }, ListModels: func(string) ([]string, error) { return nil, assertErr("boom") }},
	}
	items, err := ListModelsForShim("gamma", "/tmp/work", true)
	if err != nil || len(items) != 0 {
		t.Fatalf("unexpected ignore_errors result: %#v %v", items, err)
	}
	if _, err := ListModelsForShim("gamma", "/tmp/work", false); err == nil {
		t.Fatal("expected error")
	}
}

func TestLoadCodexModelListPrefersCacheAndDedupes(t *testing.T) {
	tmp := t.TempDir()
	oldAuth, oldCache := CodexAuthPath, CodexModelsCachePath
	defer func() { CodexAuthPath, CodexModelsCachePath = oldAuth, oldCache }()
	CodexAuthPath = filepath.Join(tmp, "auth.json")
	CodexModelsCachePath = filepath.Join(tmp, "models_cache.json")
	if err := os.WriteFile(CodexAuthPath, []byte(`{"model":"fallback"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(CodexModelsCachePath, []byte(`{"models":[{"id":"zeta"},{"name":"alpha"},{"slug":"alpha"},{"model_id":"beta"}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := loadCodexModelList(); !reflect.DeepEqual(got, []string{"alpha", "beta", "zeta"}) {
		t.Fatalf("unexpected model cache list: %#v", got)
	}
}

func TestGetCodexChatGPTSessionRefreshesExpiredAccessToken(t *testing.T) {
	tmp := t.TempDir()
	oldAuth, oldClientID, oldTokenURL := CodexAuthPath, codexOAuthClientIDDefault, CodexOAuthTokenURL
	defer func() {
		CodexAuthPath, codexOAuthClientIDDefault, CodexOAuthTokenURL = oldAuth, oldClientID, oldTokenURL
	}()
	CodexAuthPath = filepath.Join(tmp, "auth.json")
	codexOAuthClientIDDefault = "client-test"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if ct := r.Header.Get("Content-Type"); !strings.Contains(ct, "application/x-www-form-urlencoded") {
			t.Fatalf("unexpected content type: %q", ct)
		}
		if err := r.ParseForm(); err != nil {
			t.Fatal(err)
		}
		if r.Form.Get("grant_type") != "refresh_token" || r.Form.Get("refresh_token") != "refresh-1" || r.Form.Get("client_id") != "client-test" {
			t.Fatalf("unexpected refresh form: %#v", r.Form)
		}
		_, _ = w.Write([]byte(`{"access_token":"header.` + base64.RawURLEncoding.EncodeToString([]byte(`{"exp":4102444800}`)) + `.sig","refresh_token":"refresh-2","id_token":"id-2"}`))
	}))
	defer server.Close()
	CodexOAuthTokenURL = server.URL
	if err := os.WriteFile(CodexAuthPath, []byte(`{"tokens":{"access_token":"header.`+base64.RawURLEncoding.EncodeToString([]byte(`{"exp":1}`))+`.sig","refresh_token":"refresh-1","account_id":"acct-1"}}`), 0o600); err != nil {
		t.Fatal(err)
	}
	session, err := GetCodexChatGPTSession(false)
	if err != nil {
		t.Fatal(err)
	}
	if session["refresh_token"] != "refresh-2" || session["account_id"] != "acct-1" || !strings.Contains(session["access_token"], ".") {
		t.Fatalf("unexpected refreshed session: %#v", session)
	}
	stored := LoadCodexAuth()
	tokens, _ := stored["tokens"].(map[string]any)
	if tokens["refresh_token"] != "refresh-2" || stored["last_refresh"] == nil {
		t.Fatalf("expected saved refresh, got %#v", stored)
	}
}

func TestCopilotCompletionClientRoutesResponsesAndChat(t *testing.T) {
	requests := []string{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests = append(requests, r.URL.Path)
		switch r.URL.Path {
		case "/models":
			_, _ = w.Write([]byte(`{"data":[{"id":"chat-only","capabilities":{"type":"chat"},"supported_endpoints":["/chat/completions"]},{"id":"resp-capable","capabilities":{"type":"chat"},"supported_endpoints":["/chat/completions","/responses"]}]}`))
		case "/chat/completions":
			_, _ = w.Write([]byte(`{"choices":[{"message":{"content":"chat-done"}}]}`))
		case "/responses":
			_, _ = w.Write([]byte(`{"output":[{"type":"message","role":"assistant","content":[{"text":"resp-done"}]}]}`))
		default:
			w.WriteHeader(404)
		}
	}))
	defer server.Close()
	oldBase := copilotBaseURL
	defer func() { copilotBaseURL = oldBase }()
	copilotBaseURL = server.URL
	client := CopilotCompletionClient("token")
	models, err := client.ListModels()
	if err != nil || !reflect.DeepEqual(models, []string{"chat-only", "resp-capable"}) {
		t.Fatalf("unexpected copilot models: %#v %v", models, err)
	}
	message, err := client.ChatCompletion("resp-capable", nil, nil, "auto")
	if err != nil || message.Content != "resp-done" {
		t.Fatalf("unexpected responses-routed result: %#v %v", message, err)
	}
	message, err = client.ChatCompletion("chat-only", nil, nil, "auto")
	if err != nil || message.Content != "chat-done" {
		t.Fatalf("unexpected chat-routed result: %#v %v", message, err)
	}
	if !reflect.DeepEqual(requests, []string{"/models", "/models", "/responses", "/chat/completions"}) {
		t.Fatalf("unexpected request sequence: %#v", requests)
	}
}

func assertErr(message string) error { return &simpleErr{message: message} }

type simpleErr struct{ message string }

func (e *simpleErr) Error() string { return e.message }
