package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/bmatcuk/doublestar/v4"
)

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
