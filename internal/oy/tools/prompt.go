package tools

import (
	"fmt"
	"strings"
)

func ToolAsk(state *State, question string, choices []string) (string, error) {
	if state == nil || !state.Interactive {
		return "", fmt.Errorf("ask is only available in interactive mode")
	}
	if len(choices) == 0 {
		return strings.TrimSpace(AskInputFunc(question)), nil
	}
	return strings.TrimSpace(SelectInputFunc(question, choices)), nil
}

func ToolTodo(state *State, todos []map[string]string) (map[string]any, error) {
	for _, item := range todos {
		status := item["status"]
		if status == "" {
			status = "pending"
			item["status"] = status
		}
		if item["id"] == "" || item["task"] == "" {
			return nil, &ValidationError{Message: "todo items require id and task"}
		}
		if status != "pending" && status != "in_progress" && status != "done" {
			return nil, &ValidationError{Message: fmt.Sprintf("invalid todo status: %s", status)}
		}
	}
	state.Todos = cloneTodos(todos)
	return map[string]any{"items": cloneTodos(state.Todos), "count": len(state.Todos)}, nil
}

func FormatTodos(todos []map[string]string) string {
	lines := make([]string, 0, len(todos))
	for _, item := range todos {
		status := item["status"]
		icon := map[string]string{"pending": "[ ]", "in_progress": "[~]", "done": "[x]"}[status]
		if icon == "" {
			icon = "[ ]"
		}
		lines = append(lines, fmt.Sprintf("%s %s: %s", icon, item["id"], item["task"]))
	}
	return strings.Join(lines, "\n")
}

func TodoPreview(todos []map[string]string) string {
	return fmt.Sprintf("count: %d\ntodos:\n  %s", len(todos), strings.Join(strings.Split(FormatTodos(todos), "\n"), "\n  "))
}
