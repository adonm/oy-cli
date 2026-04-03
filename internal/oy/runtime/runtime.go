package runtime

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"

	"github.com/BurntSushi/toml"
	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

//go:embed session_text.toml
var sessionTextRaw string

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

var ReadOnlyTools = map[string]struct{}{
	"list": {}, "read": {}, "search": {}, "sloc": {}, "webfetch": {},
}

var (
	loadSessionTextOnce sync.Once
	loadSessionTextData map[string]any
	loadSessionTextErr  error
)

func MaxContextTokens() int {
	return envInt("OY_MAX_CONTEXT_TOKENS", DefaultMaxContextTokens)
}

func MaxBashCmdBytes() int {
	return envInt("OY_MAX_BASH_CMD_BYTES", DefaultMaxBashCmdBytes)
}

func ConfigPath() string {
	if value := strings.TrimSpace(os.Getenv("OY_CONFIG")); value != "" {
		return expandUser(value)
	}
	return filepath.Join(userHomeDir(), ".config", "oy", "config.json")
}

func DefaultBudgets() Budgets {
	return deriveRuntimeBudgets(MaxContextTokens())
}

func RuntimeBudgets(messageTokens, toolOutputTokens, toolTailTokens, defaultLineLimit int) Budgets {
	return Budgets{MessageTokens: messageTokens, ToolOutputTokens: toolOutputTokens, ToolTailTokens: toolTailTokens, DefaultLineLimit: defaultLineLimit}
}

func Session(workspace, model string, interactive bool, systemPrompt, systemFile string, yolo bool, bestOf int) SessionContext {
	return SessionContext{Workspace: workspace, Model: model, Interactive: interactive, SystemPrompt: systemPrompt, SystemFile: systemFile, Yolo: yolo, BestOf: bestOf}
}

func ModelConfig(model, shim string) SavedModelConfig {
	return SavedModelConfig{Model: model, Shim: shim}
}

func ModelConfigFromModelSpec(modelSpec string) SavedModelConfig {
	shim, model := providers.SplitModelSpec(modelSpec)
	return ModelConfig(model, shim)
}

func ResolvedModel(config SavedModelConfig) string {
	if config.Model == "" {
		return ""
	}
	if strings.Contains(config.Model, ":") || config.Shim == "" {
		return config.Model
	}
	return providers.JoinModelSpec(config.Shim, config.Model)
}

func MergeModelConfig(config SavedModelConfig, base map[string]any) map[string]any {
	data := map[string]any{}
	for key, value := range base {
		data[key] = value
	}
	if config.Model != "" {
		data["model"] = config.Model
	} else {
		delete(data, "model")
	}
	if config.Shim != "" {
		data["shim"] = config.Shim
	} else {
		delete(data, "shim")
	}
	return data
}

func LoadSessionText() (map[string]any, error) {
	loadSessionTextOnce.Do(func() {
		var data map[string]any
		if _, err := toml.Decode(sessionTextRaw, &data); err != nil {
			loadSessionTextErr = err
			return
		}
		loadSessionTextData = data
	})
	return loadSessionTextData, loadSessionTextErr
}

func SessionText(values map[string]string, keys ...string) (string, error) {
	data, err := LoadSessionText()
	if err != nil {
		return "", err
	}
	node := any(data)
	for _, key := range keys {
		items, ok := node.(map[string]any)
		if !ok {
			return "", fmt.Errorf("missing session text key: %s", strings.Join(keys, "."))
		}
		next, ok := items[key]
		if !ok {
			return "", fmt.Errorf("missing session text key: %s", strings.Join(keys, "."))
		}
		node = next
	}
	text, ok := node.(string)
	if !ok {
		return "", fmt.Errorf("session text key must point to a string: %s", strings.Join(keys, "."))
	}
	for key, value := range values {
		text = strings.ReplaceAll(text, "{"+key+"}", value)
	}
	return text, nil
}

func ToolDescription(name string) string {
	text, _ := SessionText(nil, "tools", name, "description")
	return strings.TrimSpace(text)
}

func BaseSystemPrompt() string {
	text, _ := SessionText(nil, "system", "base")
	return strings.TrimSpace(text)
}

func InteractiveSystemPrompt() string {
	text, _ := SessionText(nil, "system", "interactive_suffix")
	return strings.TrimSpace(text)
}

func NonInteractiveSystemPrompt() string {
	text, _ := SessionText(nil, "system", "noninteractive_suffix")
	return strings.TrimSpace(text)
}

func AuditSystemPrompt() string {
	text, _ := SessionText(nil, "system", "audit")
	return strings.TrimSpace(text)
}

func ActiveSystemPrompt(interactive bool) string {
	suffix := NonInteractiveSystemPrompt()
	if interactive {
		suffix = InteractiveSystemPrompt()
	}
	return BaseSystemPrompt() + "\n" + suffix + "\n"
}

func AskSystemPrompt(systemPrompt string) string {
	suffix, _ := SessionText(nil, "system", "ask_suffix")
	return systemPrompt + "\n" + strings.TrimSpace(suffix) + "\n"
}

func LoadModelConfig() SavedModelConfig {
	data, ok := providers.LoadJSON(ConfigPath(), map[string]any{}).(map[string]any)
	if !ok {
		return SavedModelConfig{}
	}
	model, _ := data["model"].(string)
	shim, _ := data["shim"].(string)
	return SavedModelConfig{Model: model, Shim: shim}
}

func SaveModelConfig(modelSpec string) (SavedModelConfig, error) {
	config := ModelConfigFromModelSpec(modelSpec)
	base, _ := providers.LoadJSON(ConfigPath(), map[string]any{}).(map[string]any)
	if base == nil {
		base = map[string]any{}
	}
	if !providers.SaveJSON(ConfigPath(), MergeModelConfig(config, base)) {
		return SavedModelConfig{}, fmt.Errorf("could not save model config")
	}
	return config, nil
}

func CurrentModel(configured string) (string, error) {
	if configured != "" {
		return configured, nil
	}
	if value := strings.TrimSpace(os.Getenv("OY_MODEL")); value != "" {
		if strings.Contains(value, ":") {
			return value, nil
		}
		shim := strings.TrimSpace(os.Getenv("OY_SHIM"))
		if shim == "" {
			shim = LoadModelConfig().Shim
		}
		if shim != "" {
			return providers.JoinModelSpec(shim, value), nil
		}
		return value, nil
	}
	if saved := ResolvedModel(LoadModelConfig()); saved != "" {
		return saved, nil
	}
	return "", fmt.Errorf("no model configured")
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

func Preview(value any, limit int) string {
	text := providers.SerializeJSON(value)
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
	return "", fmt.Errorf("path traversal denied: '%s'", relative)
}

func FormatTokens(count int) string {
	if count < 1000 {
		return fmt.Sprintf("%d tokens", count)
	}
	return fmt.Sprintf("%.1fk tokens", float64(count)/1000.0)
}

func ParseDurationSeconds(value string, name string) (int, error) {
	text := strings.TrimSpace(strings.ToLower(value))
	if text == "" {
		return 0, fmt.Errorf("invalid %s=%q. use a positive duration like 3h, 90m, or 3600s", name, value)
	}
	if digitsOnly(text) {
		seconds, _ := strconv.Atoi(text)
		if seconds <= 0 {
			return 0, fmt.Errorf("invalid %s=%q. duration must be positive", name, value)
		}
		return seconds, nil
	}
	if len(text) < 2 {
		return 0, fmt.Errorf("invalid %s=%q. use a positive duration like 3h, 90m, or 3600s", name, value)
	}
	amount, err := strconv.Atoi(text[:len(text)-1])
	if err != nil || amount <= 0 {
		return 0, fmt.Errorf("invalid %s=%q. use a positive duration like 3h, 90m, or 3600s", name, value)
	}
	switch text[len(text)-1] {
	case 'h':
		return amount * 3600, nil
	case 'm':
		return amount * 60, nil
	case 's':
		return amount, nil
	default:
		return 0, fmt.Errorf("invalid %s=%q. use a positive duration like 3h, 90m, or 3600s", name, value)
	}
}

func UnattendedLimitSeconds() (int, error) {
	value := strings.TrimSpace(os.Getenv("OY_UNATTENDED_LIMIT"))
	if value == "" {
		return DefaultUnattendedLimitSeconds, nil
	}
	return ParseDurationSeconds(value, "OY_UNATTENDED_LIMIT")
}

func RalphLimitSeconds() (int, error) {
	value := strings.TrimSpace(os.Getenv("OY_RALPH_LIMIT"))
	if value == "" {
		return 3 * 3600, nil
	}
	return ParseDurationSeconds(value, "OY_RALPH_LIMIT")
}

func ListAllModelIDs(cwd string) ([]string, []string, error) {
	shims := providers.DetectAvailableShims()
	if len(shims) == 0 {
		return nil, nil, fmt.Errorf("no shims are configured")
	}
	allModels := []string{}
	warnings := []string{}
	for _, shim := range shims {
		items, err := providers.ListModelsForShim(shim, cwd, false)
		if err != nil {
			message := strings.TrimSpace(err.Error())
			if idx := strings.IndexByte(message, '\n'); idx >= 0 {
				message = message[:idx]
			}
			warnings = append(warnings, fmt.Sprintf("Could not load models from `%s`: %s", shim, message))
			continue
		}
		allModels = append(allModels, items...)
	}
	return allModels, warnings, nil
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

func digitsOnly(value string) bool {
	for _, ch := range value {
		if ch < '0' || ch > '9' {
			return false
		}
	}
	return value != ""
}

func userHomeDir() string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return "."
	}
	return home
}

func expandUser(path string) string {
	if path == "~" {
		return userHomeDir()
	}
	if strings.HasPrefix(path, "~/") {
		return filepath.Join(userHomeDir(), path[2:])
	}
	return path
}
