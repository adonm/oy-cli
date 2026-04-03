package tools

import (
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

func ToolSloc(state State, path string, exclude []string, limit int) (map[string]any, error) {
	target, err := ExistingToolTarget(state.Root, path, "sloc")
	if err != nil {
		return nil, err
	}
	type languageSummary struct {
		Language           string
		FileCount          int
		CodeCount          int
		DocumentationCount int
		EmptyCount         int
		StringCount        int
	}
	totalFiles := 0
	totalCode := 0
	totalDocs := 0
	totalEmpty := 0
	totalStrings := 0
	totalLines := 0
	languageTotals := map[string]*languageSummary{}
	fileSummaries := []map[string]any{}
	_ = walkFiles(target, func(rel, full string, d fs.DirEntry) error {
		if d.IsDir() || excluded(rel, exclude) {
			return nil
		}
		data, err := os.ReadFile(full)
		if err != nil {
			return nil
		}
		lines := splitLines(string(data))
		total := len(lines)
		empty := 0
		for _, line := range lines {
			if strings.TrimSpace(line) == "" {
				empty++
			}
		}
		code := max(total-empty, 0)
		language := guessLanguage(rel)
		totalFiles++
		totalCode += code
		totalEmpty += empty
		totalLines += total
		summary := languageTotals[language]
		if summary == nil {
			summary = &languageSummary{Language: language}
			languageTotals[language] = summary
		}
		summary.FileCount++
		summary.CodeCount += code
		summary.EmptyCount += empty
		fileSummaries = append(fileSummaries, map[string]any{
			"path":                filepath.ToSlash(rel),
			"language":            language,
			"code_count":          code,
			"documentation_count": 0,
			"empty_count":         empty,
			"string_count":        0,
			"line_count":          total,
		})
		return nil
	})
	languages := make([]map[string]any, 0, len(languageTotals))
	for _, summary := range languageTotals {
		languages = append(languages, map[string]any{
			"language":            summary.Language,
			"file_count":          summary.FileCount,
			"code_count":          summary.CodeCount,
			"documentation_count": summary.DocumentationCount,
			"empty_count":         summary.EmptyCount,
			"string_count":        summary.StringCount,
		})
	}
	sort.Slice(languages, func(i, j int) bool {
		if languages[i]["code_count"].(int) != languages[j]["code_count"].(int) {
			return languages[i]["code_count"].(int) > languages[j]["code_count"].(int)
		}
		if languages[i]["file_count"].(int) != languages[j]["file_count"].(int) {
			return languages[i]["file_count"].(int) > languages[j]["file_count"].(int)
		}
		return strings.ToLower(languages[i]["language"].(string)) < strings.ToLower(languages[j]["language"].(string))
	})
	sort.Slice(fileSummaries, func(i, j int) bool {
		if fileSummaries[i]["code_count"].(int) != fileSummaries[j]["code_count"].(int) {
			return fileSummaries[i]["code_count"].(int) > fileSummaries[j]["code_count"].(int)
		}
		if fileSummaries[i]["line_count"].(int) != fileSummaries[j]["line_count"].(int) {
			return fileSummaries[i]["line_count"].(int) > fileSummaries[j]["line_count"].(int)
		}
		return fileSummaries[i]["path"].(string) < fileSummaries[j]["path"].(string)
	})
	shownLanguages := languages
	if limit > 0 && len(shownLanguages) > limit {
		shownLanguages = shownLanguages[:limit]
	}
	shownTopFiles := fileSummaries
	truncated := false
	if len(shownTopFiles) > 20 {
		shownTopFiles = shownTopFiles[:20]
		truncated = true
	}
	if len(languages) > len(shownLanguages) {
		truncated = true
	}
	payload := map[string]any{
		"path":                      path,
		"total_file_count":          totalFiles,
		"total_code_count":          totalCode,
		"total_documentation_count": totalDocs,
		"total_empty_count":         totalEmpty,
		"total_string_count":        totalStrings,
		"total_line_count":          totalLines,
		"language_count":            len(languages),
		"languages":                 shownLanguages,
		"top_file_count":            len(fileSummaries),
		"top_files":                 shownTopFiles,
		"truncated":                 truncated,
	}
	if exclude != nil {
		payload["exclude"] = exclude
	}
	return payload, nil
}
