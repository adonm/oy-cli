package tools

import (
	"archive/zip"
	"bytes"
	"fmt"
	"html"
	"io/fs"
	"net"
	"net/url"
	"os"
	"path"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/bmatcuk/doublestar/v4"
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
	AskInputFunc    = func(_ string) string { return "" }
	SelectInputFunc = func(_ string, choices []string) string {
		if len(choices) == 0 {
			return ""
		}
		return choices[0]
	}
	ApprovalPromptFunc = func(_ string, _ []string) string { return "deny" }
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
			return ToolSearch(*state, mustString(args, "pattern"), optionalString(args, "path", "."), optionalStringSlice(args, "exclude"), optionalInt(args, "limit", 200))
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

func BashPayload(command string, result providers.CommandResult) (map[string]any, string) {
	payload := map[string]any{
		"command":    command,
		"returncode": result.ReturnCode,
		"stdout":     strings.TrimSuffix(result.Stdout, "\n"),
		"stderr":     strings.TrimSuffix(result.Stderr, "\n"),
	}
	preview := fmt.Sprintf("$ %s\nexit: %d\nstdout:\n%s", command, result.ReturnCode, payload["stdout"])
	if payload["stderr"] != "" {
		preview += fmt.Sprintf("\nstderr:\n%s", payload["stderr"])
	}
	return payload, preview
}

func ToolBash(state State, command string, timeoutSeconds int) (map[string]any, string, error) {
	if len([]byte(command)) > runtime.MaxBashCmdBytes() {
		return nil, "", fmt.Errorf("command too large (%d chars); limit is %d bytes", len(command), runtime.MaxBashCmdBytes())
	}
	env := providers.CommandEnv(state.Root)
	bashPath := providers.Which("bash", env["PATH"])
	if bashPath == "" {
		return nil, "", fmt.Errorf("bash is not installed or not on PATH")
	}
	result, err := providers.RunCmd([]string{bashPath, "-c", command}, state.Root, env, timeDurationSeconds(timeoutSeconds), "")
	if err != nil {
		return nil, "", err
	}
	payload, preview := BashPayload(command, result)
	return payload, preview, nil
}

func ValidateURLSafe(raw string) error {
	parsed, err := url.Parse(raw)
	if err != nil {
		return err
	}
	if parsed.Scheme != "http" && parsed.Scheme != "https" {
		return fmt.Errorf("only http/https URLs are allowed, got: %q", parsed.Scheme)
	}
	hostname := parsed.Hostname()
	if hostname == "" {
		return fmt.Errorf("no hostname in URL: %q", raw)
	}
	for _, blocked := range []string{"localhost", "localhost.localdomain", "ip6-localhost", "ip6-loopback"} {
		if strings.EqualFold(hostname, blocked) {
			return fmt.Errorf("local addresses are not allowed: %q", hostname)
		}
	}
	ips, err := net.LookupIP(hostname)
	if err != nil {
		return fmt.Errorf("cannot resolve hostname %q: %v", hostname, err)
	}
	for _, ip := range ips {
		if ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalMulticast() || ip.IsLinkLocalUnicast() || ip.IsUnspecified() {
			return fmt.Errorf("URL resolves to non-public address (%s); private/reserved/loopback/link-local addresses are blocked", ip.String())
		}
	}
	return nil
}

func ToolWebfetch(state State, rawURL, method string, headers map[string]string, followRedirects bool, timeoutSeconds int) (map[string]any, error) {
	method = strings.ToUpper(strings.TrimSpace(method))
	if method == "" {
		method = "GET"
	}
	if _, ok := map[string]struct{}{"GET": {}, "HEAD": {}, "OPTIONS": {}}[method]; !ok {
		return nil, fmt.Errorf("Only GET, HEAD, OPTIONS methods are allowed, got: %q", method)
	}
	if err := ValidateURLSafe(rawURL); err != nil {
		return nil, err
	}
	cleanHeaders, err := sanitizeRequestHeaders(headers)
	if err != nil {
		return nil, err
	}
	response, err := ToolSessionFactory(timeDurationSeconds(timeoutSeconds), followRedirects).Request(method, rawURL, cleanHeaders, nil)
	if err != nil {
		return map[string]any{
			"method":     method,
			"url":        rawURL,
			"ok":         false,
			"error_type": errorTypeName(err),
			"message":    err.Error(),
		}, nil
	}
	if !textResponse(response) {
		return WebfetchPayload(response, method, nil, false, "binary"), nil
	}
	text := response.Text
	format := "text"
	if htmlResponse(response, text) {
		text = htmlToMarkdown(text)
		format = "markdown"
	}
	summarized, truncated := summarizeText(text, runtime.DefaultBudgets().ToolOutputTokens*8)
	return WebfetchPayload(response, method, &summarized, truncated, format), nil
}

func WebfetchPayload(response providers.ResponseAdapter, method string, text *string, truncated bool, format string) map[string]any {
	payload := map[string]any{
		"method":        method,
		"url":           response.URL,
		"status_code":   response.StatusCode,
		"reason_phrase": response.ReasonPhrase,
		"http_version":  response.HTTPVersion,
		"headers":       webfetchHeaders(response.Headers),
	}
	if text == nil {
		payload["binary"] = true
		payload["content_bytes"] = len(response.Content)
		return payload
	}
	payload["text"] = *text
	payload["format"] = format
	payload["truncated"] = truncated
	return payload
}

func ToolList(state State, pattern string, exclude []string, limit int) (map[string]any, error) {
	if strings.TrimSpace(pattern) == "" {
		pattern = "*"
	}
	matches, err := globPaths(state.Root, pattern, exclude)
	if err != nil {
		return nil, err
	}
	items := make([]string, 0, len(matches))
	for _, match := range matches {
		name := RelPath(state.Root, match)
		if info, err := os.Stat(match); err == nil && info.IsDir() {
			name += "/"
		}
		items = append(items, name)
	}
	shown := items
	truncated := false
	if limit > 0 && len(shown) > limit {
		shown = shown[:limit]
		truncated = true
	}
	payload := map[string]any{"path": pattern, "items": shown, "count": len(items), "truncated": truncated}
	if exclude != nil {
		payload["exclude"] = exclude
	}
	return payload, nil
}

func globPaths(root, pattern string, exclude []string) ([]string, error) {
	cleanPattern := strings.TrimSpace(pattern)
	if cleanPattern == "." || cleanPattern == "./" {
		entries, err := os.ReadDir(root)
		if err != nil {
			return nil, err
		}
		matches := make([]string, 0, len(entries))
		for _, entry := range entries {
			candidate := filepath.Join(root, entry.Name())
			rel := RelPath(root, candidate)
			if excluded(rel, exclude) {
				continue
			}
			matches = append(matches, candidate)
		}
		sort.Strings(matches)
		return matches, nil
	}
	if filepath.IsAbs(cleanPattern) || hasTraversal(cleanPattern) {
		return nil, fmt.Errorf("Path traversal denied: '%s'", pattern)
	}
	patternPath := filepath.Join(root, filepath.FromSlash(cleanPattern))
	matches, err := doublestar.FilepathGlob(patternPath)
	if err != nil {
		return nil, err
	}
	unique := map[string]struct{}{}
	filtered := make([]string, 0, len(matches))
	for _, match := range matches {
		resolved, err := filepath.Abs(match)
		if err != nil {
			continue
		}
		if resolved != root && !strings.HasPrefix(resolved, root+string(os.PathSeparator)) {
			continue
		}
		rel := RelPath(root, resolved)
		if excluded(rel, exclude) {
			continue
		}
		if _, ok := unique[resolved]; ok {
			continue
		}
		unique[resolved] = struct{}{}
		filtered = append(filtered, resolved)
	}
	sort.Strings(filtered)
	return filtered, nil
}

func hasTraversal(pattern string) bool {
	for _, part := range strings.Split(filepath.ToSlash(pattern), "/") {
		if part == ".." {
			return true
		}
	}
	return false
}

func ToolRead(state State, path string, offset, limit int) (map[string]any, string, error) {
	target, err := ExistingToolTarget(state.Root, path, "read")
	if err != nil {
		return nil, "", err
	}
	info, err := os.Stat(target)
	if err != nil {
		return nil, "", err
	}
	if info.IsDir() {
		return nil, "", fmt.Errorf("read path is a directory: %s", path)
	}
	data, err := os.ReadFile(target)
	if err != nil {
		return nil, "", err
	}
	lines := splitLines(string(data))
	start := max(offset-1, 0)
	end := min(start+max(limit, 1), len(lines))
	selected := ""
	if start < len(lines) {
		selected = strings.Join(lines[start:end], "\n")
	}
	payload := map[string]any{"path": path, "offset": offset, "limit": limit, "text": selected, "line_count": len(lines), "truncated": end < len(lines)}
	preview := fmt.Sprintf("path: %s\nlines: %d-%d of %d", path, offset, offset+len(splitLines(selected))-1, len(lines))
	if selected == "" {
		preview += "\n<empty file>"
	} else if strings.HasSuffix(path, ".py") {
		preview += "\ntext.python: " + selected
	} else {
		preview += "\ntext: " + selected
	}
	return payload, preview, nil
}

func ToolSearch(state State, pattern, path string, exclude []string, limit int) (map[string]any, error) {
	target, err := ExistingToolTarget(state.Root, path, "search")
	if err != nil {
		return nil, err
	}
	re, err := regexp.Compile(pattern)
	if err != nil {
		return nil, err
	}
	matches := []map[string]any{}
	_ = walkFiles(target, func(rel, full string, d fs.DirEntry) error {
		if d.IsDir() || excluded(rel, exclude) {
			return nil
		}
		data, err := os.ReadFile(full)
		if err != nil {
			return nil
		}
		for i, line := range splitLines(string(data)) {
			loc := re.FindStringIndex(line)
			if loc == nil {
				continue
			}
			matches = append(matches, map[string]any{"path": filepath.ToSlash(rel), "line_number": i + 1, "column": loc[0] + 1, "text": line})
		}
		return nil
	})
	shown := matches
	truncated := false
	if limit > 0 && len(shown) > limit {
		shown = shown[:limit]
		truncated = true
	}
	payload := map[string]any{"pattern": pattern, "path": path, "match_count": len(matches), "matches": shown, "truncated": truncated}
	if exclude != nil {
		payload["exclude"] = exclude
	}
	return payload, nil
}

func ToolReplace(state State, pattern, replacement, path string, exclude []string, limit int) (map[string]any, error) {
	target, err := ExistingToolTarget(state.Root, path, "replace")
	if err != nil {
		return nil, err
	}
	re, err := regexp.Compile(pattern)
	if err != nil {
		return nil, err
	}
	changed := []map[string]any{}
	_ = walkFiles(target, func(rel, full string, d fs.DirEntry) error {
		if d.IsDir() || excluded(rel, exclude) {
			return nil
		}
		data, err := os.ReadFile(full)
		if err != nil {
			return nil
		}
		updated := re.ReplaceAllString(string(data), replacement)
		if updated == string(data) {
			return nil
		}
		count := len(re.FindAllStringIndex(string(data), -1))
		if err := os.WriteFile(full, []byte(updated), 0o644); err != nil {
			return nil
		}
		changed = append(changed, map[string]any{"path": filepath.ToSlash(rel), "replacements": count})
		return nil
	})
	shown := changed
	truncated := false
	if limit > 0 && len(shown) > limit {
		shown = shown[:limit]
		truncated = true
	}
	replacementCount := 0
	for _, item := range changed {
		replacementCount += item["replacements"].(int)
	}
	payload := map[string]any{"pattern": pattern, "replacement": replacement, "path": path, "changed_file_count": len(changed), "replacement_count": replacementCount, "changed_files": shown, "truncated": truncated}
	if exclude != nil {
		payload["exclude"] = exclude
	}
	return payload, nil
}

func ToolSloc(state State, path string, exclude []string, limit int) (map[string]any, error) {
	target, err := ExistingToolTarget(state.Root, path, "sloc")
	if err != nil {
		return nil, err
	}
	totalFiles := 0
	totalCode := 0
	pythonFiles := []map[string]any{}
	_ = walkFiles(target, func(rel, full string, d fs.DirEntry) error {
		if d.IsDir() || excluded(rel, exclude) {
			return nil
		}
		data, err := os.ReadFile(full)
		if err != nil {
			return nil
		}
		lines := splitLines(string(data))
		code := 0
		for _, line := range lines {
			if strings.TrimSpace(line) != "" {
				code++
			}
		}
		totalFiles++
		totalCode += code
		if strings.HasSuffix(rel, ".py") {
			pythonFiles = append(pythonFiles, map[string]any{"path": filepath.ToSlash(rel), "language": "Python", "code_count": code, "documentation_count": 0, "empty_count": max(len(lines)-code, 0), "string_count": 0, "line_count": len(lines)})
		}
		return nil
	})
	sort.Slice(pythonFiles, func(i, j int) bool {
		return pythonFiles[i]["code_count"].(int) > pythonFiles[j]["code_count"].(int)
	})
	topFiles := pythonFiles
	truncated := false
	if len(topFiles) > 20 {
		topFiles = topFiles[:20]
		truncated = true
	}
	payload := map[string]any{
		"path":                      path,
		"total_file_count":          totalFiles,
		"total_code_count":          totalCode,
		"total_documentation_count": 0,
		"total_empty_count":         0,
		"total_string_count":        0,
		"total_line_count":          totalCode,
		"language_count":            1,
		"languages":                 []map[string]any{{"language": "Python", "file_count": len(pythonFiles), "code_count": totalCode, "documentation_count": 0, "empty_count": 0, "string_count": 0}},
		"top_file_count":            len(pythonFiles),
		"top_files":                 topFiles,
		"truncated":                 truncated,
	}
	if exclude != nil {
		payload["exclude"] = exclude
	}
	return payload, nil
}

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
		"pattern": map[string]any{"type": "string"},
		"path":    map[string]any{"type": "string"},
		"exclude": map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
		"limit":   map[string]any{"type": "integer"},
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

func errorTypeName(err error) string {
	if err == nil {
		return ""
	}
	name := reflect.TypeOf(err).String()
	name = strings.TrimPrefix(name, "*")
	if idx := strings.LastIndex(name, "."); idx >= 0 {
		name = name[idx+1:]
	}
	return name
}

func isMissing(value any) bool {
	if value == nil {
		return true
	}
	if text, ok := value.(string); ok {
		return strings.TrimSpace(text) == ""
	}
	return false
}

func mustString(args map[string]any, key string) string {
	if value, ok := args[key].(string); ok {
		return value
	}
	return ""
}

func optionalString(args map[string]any, key, fallback string) string {
	if value, ok := args[key].(string); ok && value != "" {
		return value
	}
	return fallback
}

func optionalInt(args map[string]any, key string, fallback int) int {
	switch value := args[key].(type) {
	case int:
		return value
	case float64:
		return int(value)
	default:
		return fallback
	}
}

func optionalBool(args map[string]any, key string, fallback bool) bool {
	if value, ok := args[key].(bool); ok {
		return value
	}
	return fallback
}

func optionalStringSlice(args map[string]any, key string) []string {
	value, ok := args[key]
	if !ok || value == nil {
		return nil
	}
	switch items := value.(type) {
	case []string:
		return append([]string(nil), items...)
	case []any:
		out := make([]string, 0, len(items))
		for _, item := range items {
			if text, ok := item.(string); ok {
				out = append(out, text)
			}
		}
		return out
	default:
		return nil
	}
}

func optionalStringMap(args map[string]any, key string) map[string]string {
	value, ok := args[key]
	if !ok || value == nil {
		return nil
	}
	out := map[string]string{}
	switch items := value.(type) {
	case map[string]string:
		for k, v := range items {
			out[k] = v
		}
	case map[string]any:
		for k, v := range items {
			if text, ok := v.(string); ok {
				out[k] = text
			}
		}
	}
	return out
}

func mustTodos(value any) []map[string]string {
	items, _ := value.([]map[string]string)
	if items != nil {
		return cloneTodos(items)
	}
	rows, _ := value.([]any)
	out := make([]map[string]string, 0, len(rows))
	for _, row := range rows {
		entry := map[string]string{}
		switch item := row.(type) {
		case map[string]string:
			for k, v := range item {
				entry[k] = v
			}
		case map[string]any:
			for k, v := range item {
				if text, ok := v.(string); ok {
					entry[k] = text
				}
			}
		}
		out = append(out, entry)
	}
	return out
}

func cloneTodos(todos []map[string]string) []map[string]string {
	out := make([]map[string]string, 0, len(todos))
	for _, item := range todos {
		copied := map[string]string{}
		for key, value := range item {
			copied[key] = value
		}
		out = append(out, copied)
	}
	return out
}

func excluded(pathname string, patterns []string) bool {
	normalizedPath := strings.TrimPrefix(filepath.ToSlash(pathname), "./")
	for _, pattern := range patterns {
		normalizedPattern := strings.TrimSpace(filepath.ToSlash(pattern))
		if normalizedPattern == "" {
			continue
		}
		if ok, _ := path.Match(normalizedPattern, normalizedPath); ok {
			return true
		}
		if strings.HasSuffix(normalizedPattern, "/**") {
			prefix := strings.TrimSuffix(normalizedPattern, "/**")
			if normalizedPath == prefix || strings.HasPrefix(normalizedPath, prefix+"/") {
				return true
			}
		}
	}
	return false
}

func splitLines(text string) []string {
	text = strings.TrimSuffix(text, "\n")
	if text == "" {
		return []string{}
	}
	return strings.Split(text, "\n")
}

func walkFiles(root string, fn func(rel, full string, d fs.DirEntry) error) error {
	info, err := os.Stat(root)
	if err != nil {
		return err
	}
	if !info.IsDir() {
		entry, err := os.ReadDir(filepath.Dir(root))
		if err != nil {
			return err
		}
		for _, item := range entry {
			if filepath.Join(filepath.Dir(root), item.Name()) == root {
				return fn(filepath.Base(root), root, item)
			}
		}
		return nil
	}
	return filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if path == root {
			return nil
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		return fn(filepath.ToSlash(rel), path, d)
	})
}

func sanitizeRequestHeaders(headers map[string]string) (map[string]string, error) {
	if len(headers) == 0 {
		return map[string]string{}, nil
	}
	blocked := map[string]struct{}{
		"authorization": {}, "cookie": {}, "host": {}, "proxy-authorization": {}, "x-forwarded-for": {}, "x-real-ip": {},
	}
	clean := map[string]string{}
	for key, value := range headers {
		if _, ok := blocked[strings.ToLower(key)]; ok {
			return nil, fmt.Errorf("Header %q is not allowed in webfetch requests", key)
		}
		if strings.Contains(value, "\n") || strings.Contains(value, "\r") {
			return nil, fmt.Errorf("Header value for %q contains invalid CRLF characters", key)
		}
		clean[key] = value
	}
	return clean, nil
}

func textResponse(response providers.ResponseAdapter) bool {
	contentType := strings.ToLower(strings.TrimSpace(strings.Split(response.Headers["content-type"], ";")[0]))
	if contentType == "" || strings.HasPrefix(contentType, "text/") {
		return true
	}
	if strings.HasSuffix(contentType, "+json") || strings.HasSuffix(contentType, "+xml") {
		return true
	}
	switch contentType {
	case "application/json", "application/xml", "application/javascript", "application/x-javascript", "application/x-www-form-urlencoded", "image/svg+xml", "text/html", "application/xhtml+xml":
		return true
	default:
		return false
	}
}

func htmlResponse(response providers.ResponseAdapter, text string) bool {
	contentType := strings.ToLower(strings.TrimSpace(strings.Split(response.Headers["content-type"], ";")[0]))
	if contentType == "text/html" || contentType == "application/xhtml+xml" {
		return true
	}
	trimmed := strings.ToLower(strings.TrimSpace(text))
	return strings.HasPrefix(trimmed, "<!doctype html") || strings.HasPrefix(trimmed, "<html")
}

func htmlToMarkdown(text string) string {
	replacements := []struct{ pattern, replacement string }{
		{`(?is)<h1[^>]*>(.*?)</h1>`, "$1\n=====\n\n"},
		{`(?is)<h2[^>]*>(.*?)</h2>`, "$1\n-----\n\n"},
		{`(?is)<a[^>]*href=['\"]([^'\"]+)['\"][^>]*>(.*?)</a>`, `[$2]($1)`},
		{`(?is)<p[^>]*>(.*?)</p>`, "$1\n\n"},
		{`(?is)<br\s*/?>`, "\n"},
	}
	out := text
	for _, item := range replacements {
		out = regexp.MustCompile(item.pattern).ReplaceAllString(out, item.replacement)
	}
	out = regexp.MustCompile(`(?is)<[^>]+>`).ReplaceAllString(out, "")
	out = html.UnescapeString(out)
	out = regexp.MustCompile(`\n{3,}`).ReplaceAllString(out, "\n\n")
	return strings.TrimSpace(out)
}

func summarizeText(text string, limit int) (string, bool) {
	if limit <= 0 || len(text) <= limit {
		return text, false
	}
	if limit <= 3 {
		return text[:limit], true
	}
	return text[:limit-3] + "...", true
}

func webfetchHeaders(headers map[string]string) map[string]string {
	out := map[string]string{}
	for key, value := range headers {
		name := httpHeaderName(key)
		if strings.EqualFold(key, "location") || strings.EqualFold(key, "set-cookie") || strings.EqualFold(key, "www-authenticate") || strings.EqualFold(key, "proxy-authenticate") {
			out[name] = "<redacted>"
			continue
		}
		out[name] = value
	}
	return out
}

func httpHeaderName(name string) string {
	parts := strings.Split(strings.ToLower(name), "-")
	for i, part := range parts {
		if part == "" {
			continue
		}
		parts[i] = strings.ToUpper(part[:1]) + part[1:]
	}
	return strings.Join(parts, "-")
}

func timeDurationSeconds(seconds int) time.Duration {
	if seconds <= 0 {
		seconds = 120
	}
	return time.Duration(seconds) * time.Second
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}

var _ = bytes.MinRead
var _ = zip.ErrFormat
