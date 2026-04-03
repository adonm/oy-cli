package providers

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

var (
	CodexAuthPath        = filepath.Join(userHomeDir(), ".codex", "auth.json")
	CodexModelsCachePath = filepath.Join(userHomeDir(), ".codex", "models_cache.json")
	OpencodeAuthPath     = filepath.Join(userHomeDir(), ".local", "share", "opencode", "auth.json")
)

type CommandResult struct {
	ReturnCode int
	Stdout     string
	Stderr     string
}

var (
	commandEnvOnce sync.Once
	commandEnvData map[string]string
)

func LoadJSON(path string, defaultValue any) any {
	data, err := os.ReadFile(path)
	if err != nil {
		return defaultValue
	}
	var decoded any
	if err := json.Unmarshal(data, &decoded); err != nil {
		return defaultValue
	}
	return decoded
}

func loadJSONObject(path string) map[string]any {
	data, ok := LoadJSON(path, map[string]any{}).(map[string]any)
	if !ok {
		return map[string]any{}
	}
	return data
}

func EnsurePrivateDir(path string) error {
	if err := os.MkdirAll(path, 0o700); err != nil {
		return err
	}
	return os.Chmod(path, 0o700)
}

func SaveJSON(path string, data any) bool {
	if err := EnsurePrivateDir(filepath.Dir(path)); err != nil {
		return false
	}
	encoded, err := json.MarshalIndent(data, "", "  ")
	if err != nil {
		return false
	}
	encoded = append(encoded, '\n')
	if err := os.WriteFile(path, encoded, 0o600); err != nil {
		return false
	}
	return os.Chmod(path, 0o600) == nil
}

func Which(command, path string) string {
	if path == "" {
		resolved, _ := exec.LookPath(command)
		return resolved
	}
	for _, dir := range filepath.SplitList(path) {
		candidate := filepath.Join(dir, command)
		if info, err := os.Stat(candidate); err == nil && !info.IsDir() {
			return candidate
		}
	}
	return ""
}

func RunCmd(cmd []string, cwd string, env map[string]string, timeout time.Duration, stdinText string) (CommandResult, error) {
	if len(cmd) == 0 {
		return CommandResult{}, errors.New("command must not be empty")
	}
	if timeout <= 0 {
		timeout = time.Second
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	execCmd := exec.CommandContext(ctx, cmd[0], cmd[1:]...)
	if cwd != "" {
		execCmd.Dir = cwd
	}
	if env != nil {
		execCmd.Env = flattenEnv(env)
	}
	if stdinText != "" {
		execCmd.Stdin = strings.NewReader(stdinText)
	}
	var stdout, stderr bytes.Buffer
	execCmd.Stdout = &stdout
	execCmd.Stderr = &stderr
	err := execCmd.Run()
	result := CommandResult{Stdout: stdout.String(), Stderr: stderr.String()}
	if execCmd.ProcessState != nil {
		result.ReturnCode = execCmd.ProcessState.ExitCode()
	}
	if ctx.Err() == context.DeadlineExceeded {
		return result, fmt.Errorf("command timed out after %s", timeout)
	}
	if err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) {
			return result, nil
		}
		return result, err
	}
	return result, nil
}

func CommandEnv(_ string) map[string]string {
	commandEnvOnce.Do(func() {
		commandEnvData = make(map[string]string)
		for _, item := range os.Environ() {
			key, value, ok := strings.Cut(item, "=")
			if ok {
				commandEnvData[key] = value
			}
		}
	})
	out := make(map[string]string, len(commandEnvData))
	for key, value := range commandEnvData {
		out[key] = value
	}
	return out
}

func ResetCommandEnvCache() {
	commandEnvOnce = sync.Once{}
	commandEnvData = nil
}

func DefaultRegion(choice string) string {
	for _, value := range []string{choice, os.Getenv("AWS_REGION"), os.Getenv("AWS_DEFAULT_REGION"), "ap-southeast-2"} {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return "ap-southeast-2"
}

func LoadCodexAuth() map[string]any {
	return loadJSONObject(CodexAuthPath)
}

func LoadCodexSession() (map[string]any, error) {
	auth := LoadCodexAuth()
	if len(auth) == 0 {
		return nil, fmt.Errorf("Codex CLI credentials were not found in ~/.codex/auth.json")
	}
	if apiKey, _ := auth["OPENAI_API_KEY"].(string); apiKey != "" {
		return auth, nil
	}
	tokens, _ := auth["tokens"].(map[string]any)
	for _, key := range []string{"access_token", "refresh_token"} {
		if token, _ := tokens[key].(string); token != "" {
			return auth, nil
		}
	}
	return nil, fmt.Errorf("Codex CLI auth file does not contain a usable session")
}

func DecodeJWTExpiryEpoch(token string) *float64 {
	parts := strings.Split(token, ".")
	if len(parts) < 2 {
		return nil
	}
	payload := parts[1]
	for len(payload)%4 != 0 {
		payload += "="
	}
	decoded, err := base64.URLEncoding.DecodeString(payload)
	if err != nil {
		return nil
	}
	var data map[string]any
	if err := json.Unmarshal(decoded, &data); err != nil {
		return nil
	}
	switch v := data["exp"].(type) {
	case float64:
		return &v
	case int:
		f := float64(v)
		return &f
	default:
		return nil
	}
}

func OpencodeAPIKey(name string) string {
	entry, _ := loadJSONObject(OpencodeAuthPath)[name].(map[string]any)
	key, _ := entry["key"].(string)
	return key
}

func GetGitHubToken() string {
	for _, name := range []string{"COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"} {
		if value := strings.TrimSpace(os.Getenv(name)); value != "" {
			return value
		}
	}
	gh := Which("gh", os.Getenv("PATH"))
	if gh == "" {
		return ""
	}
	result, err := RunCmd([]string{gh, "auth", "token"}, "", nil, 10*time.Second, "")
	if err != nil || result.ReturnCode != 0 {
		return ""
	}
	return strings.TrimSpace(result.Stdout)
}

func flattenEnv(env map[string]string) []string {
	items := make([]string, 0, len(env))
	for key, value := range env {
		items = append(items, key+"="+value)
	}
	return items
}

func userHomeDir() string {
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return "."
	}
	return home
}
