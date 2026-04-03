package tools

import (
	"archive/zip"
	"bytes"
	"fmt"
	"io/fs"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
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

type State struct {
	Root                    string
	Interactive             bool
	Yolo                    bool
	ApproveAllMutatingTools bool
	Todos                   []map[string]string
}

func DefaultListLimit() int {
	return runtime.DefaultBudgets().DefaultLineLimit
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

func ToolTodo(state *State, todos []map[string]string) (map[string]any, error) {
	for _, item := range todos {
		status := item["status"]
		if status == "" {
			status = "pending"
			item["status"] = status
		}
		if item["id"] == "" || item["task"] == "" {
			return nil, fmt.Errorf("todo items require id and task")
		}
		if status != "pending" && status != "in_progress" && status != "done" {
			return nil, fmt.Errorf("invalid todo status: %s", status)
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
	if pattern == "" {
		pattern = "*"
	}
	items := []string{}
	entries, err := os.ReadDir(state.Root)
	if err != nil {
		return nil, err
	}
	for _, entry := range entries {
		name := entry.Name()
		if pattern != "*" {
			matched, err := filepath.Match(pattern, name)
			if err != nil || !matched {
				continue
			}
		}
		if excluded(name, exclude) {
			continue
		}
		if entry.IsDir() {
			name += "/"
		}
		items = append(items, name)
	}
	sort.Strings(items)
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

func excluded(path string, patterns []string) bool {
	for _, pattern := range patterns {
		if ok, _ := filepath.Match(pattern, path); ok {
			return true
		}
		if strings.HasSuffix(pattern, "/**") && strings.HasPrefix(path, strings.TrimSuffix(pattern, "/**")) {
			return true
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
