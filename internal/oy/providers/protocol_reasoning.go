package providers

import (
	"strings"
	"sync"
)

type ReasoningCache struct {
	mu    sync.Mutex
	items map[string]bool
}

var reasoningSupport = ReasoningCache{items: map[string]bool{}}

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
