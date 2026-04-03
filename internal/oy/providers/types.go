package providers

import (
	"encoding/json"
	"fmt"
)

type JSONLike = any

type ToolCall struct {
	ID        string         `json:"id"`
	Name      string         `json:"name"`
	Arguments map[string]any `json:"arguments,omitempty"`
}

type ToolResult struct {
	OK      bool     `json:"ok"`
	Content JSONLike `json:"content,omitempty"`
}

type ChatMessage struct {
	Role              string            `json:"role"`
	Content           string            `json:"content,omitempty"`
	ToolCalls         []ToolCall        `json:"tool_calls,omitempty"`
	ThoughtSignatures map[string]string `json:"thought_signatures,omitempty"`
	ToolCallID        string            `json:"tool_call_id,omitempty"`
	Name              string            `json:"name,omitempty"`
}

type ResponseAdapter struct {
	StatusCode   int               `json:"status_code"`
	Headers      map[string]string `json:"headers"`
	Text         string            `json:"text"`
	Content      []byte            `json:"content"`
	URL          string            `json:"url"`
	ReasonPhrase string            `json:"reason_phrase"`
	HTTPVersion  string            `json:"http_version"`
}

type CompletionClient interface {
	ChatCompletion(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error)
	ListModels() ([]string, error)
}

func NormalizeJSONLike(value any) JSONLike {
	switch v := value.(type) {
	case nil, string, bool, float32, float64, int, int8, int16, int32, int64, uint, uint8, uint16, uint32, uint64:
		return v
	case map[string]any:
		out := make(map[string]any, len(v))
		for key, item := range v {
			out[key] = NormalizeJSONLike(item)
		}
		return out
	case []any:
		out := make([]any, 0, len(v))
		for _, item := range v {
			out = append(out, NormalizeJSONLike(item))
		}
		return out
	case []string:
		out := make([]any, 0, len(v))
		for _, item := range v {
			out = append(out, item)
		}
		return out
	default:
		return fmt.Sprint(v)
	}
}

func SerializeJSON(value any) string {
	normalized := NormalizeJSONLike(value)
	if text, ok := normalized.(string); ok {
		return text
	}
	data, err := json.Marshal(normalized)
	if err != nil {
		return fmt.Sprint(normalized)
	}
	return string(data)
}

func ToolCallMessage(id, name string, arguments map[string]any) ToolCall {
	if arguments == nil {
		arguments = map[string]any{}
	}
	return ToolCall{ID: id, Name: name, Arguments: arguments}
}

func NewToolResult(ok bool, content JSONLike) ToolResult {
	return ToolResult{OK: ok, Content: content}
}

func SystemMessage(content string) ChatMessage {
	return ChatMessage{Role: "system", Content: content}
}

func UserMessage(content string) ChatMessage {
	return ChatMessage{Role: "user", Content: content}
}

func AssistantMessage(content string, toolCalls []ToolCall) ChatMessage {
	return ChatMessage{Role: "assistant", Content: content, ToolCalls: toolCalls}
}

func ToolMessage(toolCallID, name string, content ToolResult) ChatMessage {
	return ChatMessage{Role: "tool", ToolCallID: toolCallID, Name: name, Content: SerializeJSON(content)}
}

func SplitModelSpec(spec string) (string, string) {
	for _, shim := range ShimOrder {
		prefix := shim + ":"
		if len(spec) > len(prefix) && spec[:len(prefix)] == prefix {
			return shim, spec[len(prefix):]
		}
	}
	return "", spec
}

func JoinModelSpec(shim, model string) string {
	return shim + ":" + model
}

const (
	ShimOpenAI     = "openai"
	ShimCodex      = "codex"
	ShimMantle     = "bedrock-mantle"
	ShimCopilot    = "copilot"
	ShimOpenCode   = "opencode"
	ShimOpenCodeGo = "opencode-go"
)

var ShimOrder = []string{
	ShimOpenAI,
	ShimCodex,
	ShimMantle,
	ShimCopilot,
	ShimOpenCode,
	ShimOpenCodeGo,
}

func ValidateShim(shim string) error {
	for _, item := range ShimOrder {
		if item == shim {
			return nil
		}
	}
	return fmt.Errorf("unknown shim value: %q", shim)
}

func DetectAvailableShims() []string {
	return append([]string(nil), ShimOrder...)
}
