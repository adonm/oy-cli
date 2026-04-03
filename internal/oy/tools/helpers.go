package tools

import (
	"fmt"
	"html"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

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

func guessLanguage(path string) string {
	ext := strings.ToLower(filepath.Ext(path))
	if name, ok := map[string]string{
		".go":   "Go",
		".py":   "Python",
		".md":   "Markdown",
		".txt":  "Text",
		".toml": "TOML",
		".yaml": "YAML",
		".yml":  "YAML",
		".json": "JSON",
		".sh":   "Shell",
		".js":   "JavaScript",
		".ts":   "TypeScript",
		".html": "HTML",
		".css":  "CSS",
		".xml":  "XML",
	}[ext]; ok {
		return name
	}
	if ext == "" {
		return "Text"
	}
	return strings.ToUpper(strings.TrimPrefix(ext, "."))
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
