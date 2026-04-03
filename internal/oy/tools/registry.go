package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
)

type Spec struct {
	Name        string         `json:"name"`
	Description string         `json:"description"`
	Parameters  map[string]any `json:"parameters"`
	Mutating    bool           `json:"mutating,omitempty"`
}

type RegisteredTool struct {
	Spec
	Required []string
	Handler  func(state *State, args map[string]any) (any, error)
}

type State struct {
	Root                    string
	Interactive             bool
	Yolo                    bool
	ApproveAllMutatingTools bool
	Todos                   []map[string]string
}

type HTTPRequester interface {
	Request(method, url string, headers map[string]string, body []byte) (providers.ResponseAdapter, error)
}

type ValidationError struct{ Message string }

func (e *ValidationError) Error() string { return e.Message }

type PermissionError struct{ Message string }

func (e *PermissionError) Error() string { return e.Message }

var (
	AskFunc = func(_ *State, _ string, choices []string) (string, error) {
		if len(choices) == 0 {
			return "", nil
		}
		return choices[0], nil
	}
	ApprovalPromptFunc = func(_ *State, _ string, _ []string) (string, error) { return "deny", nil }
	ToolSessionFactory = func(timeout time.Duration, followRedirects bool) HTTPRequester {
		return providers.ToolSession(timeout, followRedirects)
	}
)

var ToolRegistry = map[string]RegisteredTool{
	"ask": {
		Spec:     Spec{Name: "ask", Description: runtime.ToolDescription("ask"), Parameters: askSchema()},
		Required: []string{"question"},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolAsk(state, mustString(args, "question"), optionalStringSlice(args, "choices"))
		},
	},
	"todo": {
		Spec:     Spec{Name: "todo", Description: runtime.ToolDescription("todo"), Parameters: todoSchema()},
		Required: []string{"todos"},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolTodo(state, mustTodos(args["todos"]))
		},
	},
	"bash": {
		Spec:     Spec{Name: "bash", Description: runtime.ToolDescription("bash"), Parameters: bashSchema(), Mutating: true},
		Required: []string{"command"},
		Handler: func(state *State, args map[string]any) (any, error) {
			payload, _, err := ToolBash(*state, mustString(args, "command"), optionalInt(args, "timeout_seconds", 120))
			return payload, err
		},
	},
	"webfetch": {
		Spec:     Spec{Name: "webfetch", Description: runtime.ToolDescription("webfetch"), Parameters: webfetchSchema()},
		Required: []string{"url"},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolWebfetch(
				*state,
				mustString(args, "url"),
				optionalString(args, "method", "GET"),
				optionalStringMap(args, "headers"),
				optionalBool(args, "follow_redirects", false),
				optionalInt(args, "timeout_seconds", int(providers.DefaultWebfetchTimeoutSeconds/time.Second)),
			)
		},
	},
	"list": {
		Spec: Spec{Name: "list", Description: runtime.ToolDescription("list"), Parameters: listSchema()},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolList(*state, optionalString(args, "path", "*"), optionalStringSlice(args, "exclude"), optionalInt(args, "limit", DefaultListLimit()))
		},
	},
	"read": {
		Spec:     Spec{Name: "read", Description: runtime.ToolDescription("read"), Parameters: readSchema()},
		Required: []string{"path"},
		Handler: func(state *State, args map[string]any) (any, error) {
			payload, _, err := ToolRead(*state, mustString(args, "path"), optionalInt(args, "offset", 1), optionalInt(args, "limit", DefaultListLimit()))
			return payload, err
		},
	},
	"search": {
		Spec:     Spec{Name: "search", Description: runtime.ToolDescription("search"), Parameters: searchSchema()},
		Required: []string{"pattern"},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolSearch(
				*state,
				mustString(args, "pattern"),
				optionalString(args, "path", "."),
				optionalString(args, "fuzzy", ""),
				optionalBool(args, "best_match", false),
				optionalBool(args, "enhance_match", false),
				optionalStringSlice(args, "exclude"),
				optionalInt(args, "limit", 200),
			)
		},
	},
	"replace": {
		Spec:     Spec{Name: "replace", Description: runtime.ToolDescription("replace"), Parameters: replaceSchema(), Mutating: true},
		Required: []string{"pattern", "replacement"},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolReplace(*state, mustString(args, "pattern"), mustString(args, "replacement"), optionalString(args, "path", "."), optionalStringSlice(args, "exclude"), optionalInt(args, "limit", 200))
		},
	},
	"sloc": {
		Spec: Spec{Name: "sloc", Description: runtime.ToolDescription("sloc"), Parameters: slocSchema()},
		Handler: func(state *State, args map[string]any) (any, error) {
			return ToolSloc(*state, optionalString(args, "path", "."), optionalStringSlice(args, "exclude"), optionalInt(args, "limit", 200))
		},
	},
}

func DefaultListLimit() int {
	return runtime.DefaultBudgets().DefaultLineLimit
}

func ToolSpecs(registry map[string]RegisteredTool) []map[string]any {
	if registry == nil {
		registry = ToolRegistry
	}
	names := make([]string, 0, len(registry))
	for name := range registry {
		names = append(names, name)
	}
	sort.Strings(names)
	items := make([]map[string]any, 0, len(names))
	for _, name := range names {
		tool := registry[name]
		items = append(items, map[string]any{
			"name":        tool.Name,
			"description": tool.Description,
			"parameters":  tool.Parameters,
		})
	}
	return items
}

func SelectTools(include, exclude map[string]struct{}) map[string]RegisteredTool {
	selected := map[string]RegisteredTool{}
	for name, tool := range ToolRegistry {
		if include != nil {
			if _, ok := include[name]; !ok {
				continue
			}
		}
		if exclude != nil {
			if _, ok := exclude[name]; ok {
				continue
			}
		}
		selected[name] = tool
	}
	return selected
}

func ActiveToolRegistry(interactive bool) map[string]RegisteredTool {
	if interactive {
		return SelectTools(nil, nil)
	}
	return SelectTools(nil, map[string]struct{}{"ask": {}})
}

func ReadOnlyToolRegistry() map[string]RegisteredTool {
	return SelectTools(runtime.ReadOnlyTools, nil)
}

func InvokeTool(registry map[string]RegisteredTool, state *State, name string, args map[string]any) providers.ToolResult {
	if args == nil {
		args = map[string]any{}
	}
	tool, ok := registry[name]
	if !ok {
		return providers.NewToolResult(false, fmt.Sprintf("Tool '%s' unavailable", name))
	}
	for _, key := range tool.Required {
		if value, ok := args[key]; !ok || isMissing(value) {
			return providers.NewToolResult(false, errorPayload(&ValidationError{Message: fmt.Sprintf("missing required argument: %s", key)}))
		}
	}
	if tool.Mutating && !approveMutatingTool(state, name, args) {
		return providers.NewToolResult(false, errorPayload(&PermissionError{Message: fmt.Sprintf("tool %s denied", name)}))
	}
	payload, err := tool.Handler(state, args)
	if err != nil {
		return providers.NewToolResult(false, errorPayload(err))
	}
	return providers.NewToolResult(true, payload)
}

func CountText(count int, singular, plural string) string {
	if plural == "" {
		plural = singular + "s"
	}
	if count == 1 {
		return fmt.Sprintf("%d %s", count, singular)
	}
	return fmt.Sprintf("%d %s", count, plural)
}

func RelPath(root, path string) string {
	rel, err := filepath.Rel(root, path)
	if err != nil {
		return path
	}
	return filepath.ToSlash(rel)
}

func ExistingToolTarget(root, path, tool string) (string, error) {
	target, err := runtime.ResolvePath(root, path)
	if err != nil {
		return "", err
	}
	if _, err := os.Stat(target); err != nil {
		return "", fmt.Errorf("%s path does not exist: %s", tool, RelPath(root, target))
	}
	return target, nil
}
