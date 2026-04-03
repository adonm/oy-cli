package tools

import (
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
)

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
