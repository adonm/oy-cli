package tools

import (
	"fmt"
	"sort"
	"strings"

	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
)

func askSchema() map[string]any {
	return closedObject(
		map[string]any{
			"question": map[string]any{"type": "string"},
			"choices":  map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
		},
		[]string{"question"},
	)
}

func todoSchema() map[string]any {
	return closedObject(
		map[string]any{
			"todos": map[string]any{
				"type": "array",
				"items": map[string]any{
					"type":                 "object",
					"additionalProperties": false,
					"properties": map[string]any{
						"id":     map[string]any{"type": "string"},
						"task":   map[string]any{"type": "string"},
						"status": map[string]any{"type": "string", "enum": []string{"pending", "in_progress", "done"}},
					},
					"required": []string{"id", "task"},
				},
			},
		},
		[]string{"todos"},
	)
}

func bashSchema() map[string]any {
	return closedObject(map[string]any{
		"command":         map[string]any{"type": "string"},
		"timeout_seconds": map[string]any{"type": "integer"},
	}, []string{"command"})
}

func webfetchSchema() map[string]any {
	return closedObject(map[string]any{
		"url":              map[string]any{"type": "string"},
		"method":           map[string]any{"type": "string"},
		"headers":          map[string]any{"type": "object"},
		"follow_redirects": map[string]any{"type": "boolean"},
		"timeout_seconds":  map[string]any{"type": "integer"},
	}, []string{"url"})
}

func listSchema() map[string]any {
	return closedObject(map[string]any{
		"path":    map[string]any{"type": "string"},
		"exclude": map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
		"limit":   map[string]any{"type": "integer"},
	}, nil)
}

func readSchema() map[string]any {
	return closedObject(map[string]any{
		"path":   map[string]any{"type": "string"},
		"offset": map[string]any{"type": "integer"},
		"limit":  map[string]any{"type": "integer"},
	}, []string{"path"})
}

func searchSchema() map[string]any {
	return closedObject(map[string]any{
		"pattern":       map[string]any{"type": "string"},
		"path":          map[string]any{"type": "string"},
		"fuzzy":         map[string]any{"type": "string"},
		"best_match":    map[string]any{"type": "boolean"},
		"enhance_match": map[string]any{"type": "boolean"},
		"exclude":       map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
		"limit":         map[string]any{"type": "integer"},
	}, []string{"pattern"})
}

func replaceSchema() map[string]any {
	return closedObject(map[string]any{
		"pattern":     map[string]any{"type": "string"},
		"replacement": map[string]any{"type": "string"},
		"path":        map[string]any{"type": "string"},
		"exclude":     map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
		"limit":       map[string]any{"type": "integer"},
	}, []string{"pattern", "replacement"})
}

func slocSchema() map[string]any {
	return closedObject(map[string]any{
		"path":    map[string]any{"type": "string"},
		"exclude": map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
		"limit":   map[string]any{"type": "integer"},
	}, nil)
}

func closedObject(properties map[string]any, required []string) map[string]any {
	payload := map[string]any{
		"type":                 "object",
		"additionalProperties": false,
		"properties":           properties,
	}
	if len(required) > 0 {
		payload["required"] = required
	}
	return payload
}

func approveMutatingTool(state *State, name string, args map[string]any) bool {
	if state == nil || state.Yolo || !state.Interactive || state.ApproveAllMutatingTools {
		return true
	}
	choice := strings.TrimSpace(ApprovalPromptFunc(toolApprovalPrompt(name, args), []string{"once", "all", "deny"}))
	switch choice {
	case "all":
		state.ApproveAllMutatingTools = true
		return true
	case "once":
		return true
	default:
		return false
	}
}

func toolApprovalPrompt(name string, args map[string]any) string {
	details := []string{}
	for key, value := range args {
		if isMissing(value) {
			continue
		}
		details = append(details, fmt.Sprintf("%s: %s", strings.ReplaceAll(key, "_", "-"), runtime.Preview(value, 80)))
	}
	sort.Strings(details)
	if len(details) == 0 {
		return fmt.Sprintf("Approve `%s`?", name)
	}
	return fmt.Sprintf("Approve `%s` — %s?", name, strings.Join(details, ", "))
}

func errorPayload(err error) map[string]any {
	return map[string]any{"error_type": errorTypeName(err), "message": err.Error()}
}
