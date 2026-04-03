package runtime

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

type Budgets struct {
	MessageTokens    int
	ToolOutputTokens int
	ToolTailTokens   int
	DefaultLineLimit int
}

type SessionContext struct {
	Workspace    string
	Model        string
	Interactive  bool
	SystemPrompt string
	SystemFile   string
	Yolo         bool
	BestOf       int
}

type SavedModelConfig struct {
	Model string `json:"model,omitempty"`
	Shim  string `json:"shim,omitempty"`
}

const (
	DefaultUnattendedLimitSeconds = 3600
	DefaultSelfConsistencyBestOf  = 3
	DefaultMaxContextTokens       = 131072
	DefaultMaxBashCmdBytes        = 65536
)

var (
	MaxContextTokens = envInt("OY_MAX_CONTEXT_TOKENS", DefaultMaxContextTokens)
	MaxBashCmdBytes  = envInt("OY_MAX_BASH_CMD_BYTES", DefaultMaxBashCmdBytes)
	ConfigPath       = filepath.Join(userHomeDir(), ".config", "oy", "config.json")
	DefaultBudgets   = deriveRuntimeBudgets(MaxContextTokens)
)

func RuntimeBudgets(messageTokens, toolOutputTokens, toolTailTokens, defaultLineLimit int) Budgets {
	return Budgets{MessageTokens: messageTokens, ToolOutputTokens: toolOutputTokens, ToolTailTokens: toolTailTokens, DefaultLineLimit: defaultLineLimit}
}

func Session(workspace, model string, interactive bool, systemPrompt, systemFile string, yolo bool, bestOf int) SessionContext {
	return SessionContext{Workspace: workspace, Model: model, Interactive: interactive, SystemPrompt: systemPrompt, SystemFile: systemFile, Yolo: yolo, BestOf: bestOf}
}

func DefaultBestOfForModel(modelSpec string) int {
	_, model := providers.SplitModelSpec(modelSpec)
	lowered := strings.ToLower(model)
	if strings.Contains(lowered, "glm-5") || strings.Contains(lowered, "kimi-k2.5") || strings.Contains(lowered, "kimi-k2") {
		return DefaultSelfConsistencyBestOf
	}
	return 1
}

func SelfConsistencyBestOf(defaultValue int, modelSpec string) (int, error) {
	fallback := defaultValue
	if fallback <= 0 {
		fallback = DefaultBestOfForModel(modelSpec)
	}
	value := strings.TrimSpace(os.Getenv("OY_BEST_OF"))
	if value == "" {
		return fallback, nil
	}
	parsed, err := strconv.Atoi(value)
	if err != nil || parsed <= 0 {
		return 0, fmt.Errorf("invalid OY_BEST_OF=%s. use a positive integer", value)
	}
	return parsed, nil
}

func YoloEnabled(defaultValue bool) bool {
	value := strings.TrimSpace(strings.ToLower(os.Getenv("OY_YOLO")))
	if value == "" {
		return defaultValue
	}
	switch value {
	case "1", "true", "yes", "on":
		return true
	case "0", "false", "no", "off":
		return false
	default:
		return defaultValue
	}
}

func Preview(value any, limit int) string {
	var text string
	switch v := value.(type) {
	case string:
		text = v
	default:
		data, err := json.Marshal(v)
		if err != nil {
			text = fmt.Sprint(v)
		} else {
			text = string(data)
		}
	}
	text = strings.Join(strings.Fields(text), " ")
	if limit > 0 && len(text) > limit {
		return text[:limit-3] + "..."
	}
	return text
}

func ResolvePath(root, relative string) (string, error) {
	resolved := filepath.Clean(filepath.Join(root, relative))
	rootClean := filepath.Clean(root)
	if resolved == rootClean || strings.HasPrefix(resolved, rootClean+string(os.PathSeparator)) {
		return resolved, nil
	}
	return "", fmt.Errorf("path traversal denied: %q", relative)
}

func FormatTokens(count int) string {
	if count < 1000 {
		return fmt.Sprintf("%d tokens", count)
	}
	return fmt.Sprintf("%.1fk tokens", float64(count)/1000.0)
}

func deriveRuntimeBudgets(contextTokens int) Budgets {
	toolOutputTokens := clampInt(contextTokens/24, 2048, 8192)
	return RuntimeBudgets(
		clampInt(contextTokens/16, toolOutputTokens, 12288),
		toolOutputTokens,
		clampInt(toolOutputTokens/5, 512, 2048),
		clampInt(toolOutputTokens/6, 200, 1200),
	)
}

func clampInt(value, lower, upper int) int {
	if value < lower {
		return lower
	}
	if value > upper {
		return upper
	}
	return value
}

func envInt(name string, fallback int) int {
	value := strings.TrimSpace(os.Getenv(name))
	if value == "" {
		return fallback
	}
	parsed, err := strconv.Atoi(value)
	if err != nil {
		return fallback
	}
	return parsed
}

func userHomeDir() string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return "."
	}
	return home
}
