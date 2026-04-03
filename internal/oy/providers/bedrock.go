package providers

import (
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	aws "github.com/wagov-dtt/oy-cli/internal/oy/aws"
)

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
