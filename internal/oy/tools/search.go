package tools

import (
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

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
