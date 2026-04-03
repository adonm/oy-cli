package tools

import (
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
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

func ToolSearch(state State, pattern, path, fuzzy string, bestMatch, enhanceMatch bool, exclude []string, limit int) (map[string]any, error) {
	target, err := ExistingToolTarget(state.Root, path, "search")
	if err != nil {
		return nil, err
	}
	matcher, err := newSearchMatcher(pattern, fuzzy, bestMatch, enhanceMatch)
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
			column, ok := matcher.lineMatch(line)
			if !ok {
				continue
			}
			matches = append(matches, map[string]any{"path": filepath.ToSlash(rel), "line_number": i + 1, "column": column, "text": line})
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
	if strings.TrimSpace(fuzzy) != "" {
		payload["fuzzy"] = fuzzy
	}
	if bestMatch {
		payload["best_match"] = true
	}
	if enhanceMatch {
		payload["enhance_match"] = true
	}
	if exclude != nil {
		payload["exclude"] = exclude
	}
	return payload, nil
}

type searchMatcher struct {
	re           *regexp.Regexp
	fuzzyText    string
	fuzzyMax     int
	preferBest   bool
	fuzzyEnabled bool
}

func newSearchMatcher(pattern, fuzzy string, bestMatch, enhanceMatch bool) (searchMatcher, error) {
	matcher := searchMatcher{preferBest: bestMatch || enhanceMatch}
	if strings.TrimSpace(fuzzy) == "" {
		re, err := regexp.Compile(pattern)
		if err != nil {
			return searchMatcher{}, err
		}
		matcher.re = re
		return matcher, nil
	}
	maxDistance, err := parseFuzzyConstraint(fuzzy)
	if err != nil {
		return searchMatcher{}, err
	}
	matcher.fuzzyEnabled = true
	matcher.fuzzyText = pattern
	matcher.fuzzyMax = maxDistance
	return matcher, nil
}

func (m searchMatcher) lineMatch(line string) (int, bool) {
	if !m.fuzzyEnabled {
		loc := m.re.FindStringIndex(line)
		if loc == nil {
			return 0, false
		}
		return loc[0] + 1, true
	}
	return fuzzyLineMatch(line, m.fuzzyText, m.fuzzyMax, m.preferBest)
}

func parseFuzzyConstraint(raw string) (int, error) {
	constraint := strings.TrimSpace(raw)
	constraint = strings.TrimPrefix(constraint, "{")
	constraint = strings.TrimSuffix(constraint, "}")
	constraint = strings.TrimSpace(constraint)
	if constraint == "" {
		return 0, fmt.Errorf("fuzzy must not be empty")
	}
	for _, prefix := range []string{"s<=", "e<="} {
		if strings.HasPrefix(constraint, prefix) {
			value, err := strconv.Atoi(strings.TrimSpace(strings.TrimPrefix(constraint, prefix)))
			if err != nil || value < 0 {
				return 0, fmt.Errorf("unsupported fuzzy constraint: %q", raw)
			}
			return value, nil
		}
	}
	return 0, fmt.Errorf("unsupported fuzzy constraint: %q", raw)
}

func fuzzyLineMatch(line, pattern string, maxDistance int, preferBest bool) (int, bool) {
	patternRunes := []rune(pattern)
	lineRunes := []rune(line)
	if len(patternRunes) == 0 {
		return 1, true
	}
	bestColumn := 0
	bestDistance := maxDistance + 1
	minLen := max(len(patternRunes)-maxDistance, 1)
	maxLen := len(patternRunes) + maxDistance
	for start := 0; start < len(lineRunes); start++ {
		endLimit := min(len(lineRunes), start+maxLen)
		for end := start + minLen; end <= endLimit; end++ {
			distance := levenshteinDistance(patternRunes, lineRunes[start:end], bestDistance-1)
			if distance > maxDistance {
				continue
			}
			column := start + 1
			if !preferBest {
				return column, true
			}
			if distance < bestDistance || (distance == bestDistance && (bestColumn == 0 || column < bestColumn)) {
				bestDistance = distance
				bestColumn = column
			}
		}
	}
	if bestColumn == 0 {
		return 0, false
	}
	return bestColumn, true
}

func levenshteinDistance(a, b []rune, maxDistance int) int {
	if len(a) == 0 {
		return len(b)
	}
	if len(b) == 0 {
		return len(a)
	}
	if maxDistance >= 0 && absInt(len(a)-len(b)) > maxDistance {
		return maxDistance + 1
	}
	previous := make([]int, len(b)+1)
	current := make([]int, len(b)+1)
	for j := range previous {
		previous[j] = j
	}
	for i, ra := range a {
		current[0] = i + 1
		rowMin := current[0]
		for j, rb := range b {
			cost := 0
			if ra != rb {
				cost = 1
			}
			current[j+1] = min(min(previous[j+1]+1, current[j]+1), previous[j]+cost)
			if current[j+1] < rowMin {
				rowMin = current[j+1]
			}
		}
		if maxDistance >= 0 && rowMin > maxDistance {
			return maxDistance + 1
		}
		previous, current = current, previous
	}
	return previous[len(b)]
}

func absInt(value int) int {
	if value < 0 {
		return -value
	}
	return value
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
