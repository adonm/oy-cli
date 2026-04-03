package providers

import (
	"encoding/json"
	"fmt"
	"strings"
)

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
