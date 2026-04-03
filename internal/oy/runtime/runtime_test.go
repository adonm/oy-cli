package runtime

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestSessionTextGuidance(t *testing.T) {
	if got := BaseSystemPrompt(); !contains(got, "Never guess") || !contains(got, "`webfetch` freely") {
		t.Fatalf("unexpected base system prompt: %q", got)
	}
	if got := ActiveSystemPrompt(true); !contains(got, BaseSystemPrompt()) {
		t.Fatalf("interactive prompt missing base")
	}
	if got := ActiveSystemPrompt(false); !contains(got, BaseSystemPrompt()) {
		t.Fatalf("non-interactive prompt missing base")
	}
	if got := AskSystemPrompt("sys"); !contains(got, "no-write rather than no-network") {
		t.Fatalf("unexpected ask system prompt: %q", got)
	}
	auditPrompt := AuditSystemPrompt()
	for _, needle := range []string{"Renovate lookup report command", "pnpm dlx --allow-build=re2 renovate", "npm exec --yes --package renovate -- renovate", "--dry-run=lookup", "--report-path=renovate-report.json", "throwaway local artifact", "delete it or leave it untracked", "`jq` when available or Python otherwise"} {
		if !contains(auditPrompt, needle) {
			t.Fatalf("audit prompt missing %q", needle)
		}
	}
	for _, name := range []string{"list", "search", "replace", "sloc"} {
		if !contains(ToolDescription(name), "exclude") {
			t.Fatalf("tool description missing exclude for %s", name)
		}
	}
	if !contains(ToolDescription("webfetch"), "broad browsing") {
		t.Fatal("webfetch description missing broad browsing")
	}
	if !contains(ToolDescription("todo"), "Every item must include string `id` and string `task`") {
		t.Fatal("todo description missing requirements")
	}
}

func TestModelConfigRoundTripAndEnvOverride(t *testing.T) {
	tmp := t.TempDir()
	configPath := filepath.Join(tmp, "config.json")
	t.Setenv("OY_CONFIG", configPath)
	os.Unsetenv("OY_MODEL")
	os.Unsetenv("OY_SHIM")
	os.Unsetenv("OY_YOLO")
	os.Unsetenv("OY_BEST_OF")

	saved, err := SaveModelConfig("openai:gpt-test")
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(saved, SavedModelConfig{Model: "gpt-test", Shim: "openai"}) {
		t.Fatalf("unexpected saved config: %#v", saved)
	}
	if loaded := LoadModelConfig(); !reflect.DeepEqual(loaded, SavedModelConfig{Model: "gpt-test", Shim: "openai"}) {
		t.Fatalf("unexpected loaded config: %#v", loaded)
	}
	current, err := CurrentModel("")
	if err != nil || current != "openai:gpt-test" {
		t.Fatalf("unexpected current model: %q %v", current, err)
	}

	t.Setenv("OY_SHIM", "copilot")
	t.Setenv("OY_MODEL", "gpt-live")
	t.Setenv("OY_YOLO", "yes")
	t.Setenv("OY_BEST_OF", "5")
	current, err = CurrentModel("")
	if err != nil || current != "copilot:gpt-live" {
		t.Fatalf("unexpected env current model: %q %v", current, err)
	}
	if !YoloEnabled(false) {
		t.Fatal("expected yolo enabled")
	}
	bestOf, err := SelfConsistencyBestOf(0, "copilot:gpt-live")
	if err != nil || bestOf != 5 {
		t.Fatalf("unexpected best_of: %d %v", bestOf, err)
	}
}

func TestBestOfHelpersAndDurations(t *testing.T) {
	for _, model := range []string{"openai:glm-5", "bedrock-mantle:moonshotai.kimi-k2.5"} {
		if got := DefaultBestOfForModel(model); got != DefaultSelfConsistencyBestOf {
			t.Fatalf("unexpected best-of for %q: %d", model, got)
		}
	}
	if got := DefaultBestOfForModel("openai:gpt-5"); got != 1 {
		t.Fatalf("unexpected default for gpt-5: %d", got)
	}
	if got, err := ParseDurationSeconds("90m", "duration"); err != nil || got != 5400 {
		t.Fatalf("unexpected parsed duration: %d %v", got, err)
	}
	t.Setenv("OY_RALPH_LIMIT", "90m")
	if got, err := RalphLimitSeconds(); err != nil || got != 5400 {
		t.Fatalf("unexpected ralph limit: %d %v", got, err)
	}
	t.Setenv("OY_BEST_OF", "bad")
	if _, err := SelfConsistencyBestOf(0, "openai:glm-5"); err == nil {
		t.Fatal("expected invalid OY_BEST_OF error")
	}
}

func TestDebugLogLifecycle(t *testing.T) {
	t.Setenv("OY_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	t.Setenv("OY_DEBUG", "1")
	if err := DisableDebugLog(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = DisableDebugLog() })
	path, err := InitDebugLog()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasSuffix(path, "debug.jsonl") {
		t.Fatalf("unexpected debug path: %q", path)
	}
	DebugLog("request", map[string]any{"model": "openai:gpt-test", "step": 1})
	if err := DisableDebugLog(); err != nil {
		t.Fatal(err)
	}
	lines := strings.Split(strings.TrimSpace(string(mustReadFile(t, path))), "\n")
	if len(lines) != 1 {
		t.Fatalf("unexpected debug log lines: %#v", lines)
	}
	var payload map[string]any
	if err := json.Unmarshal([]byte(lines[0]), &payload); err != nil {
		t.Fatal(err)
	}
	if payload["event"] != "request" || payload["model"] != "openai:gpt-test" || int(payload["step"].(float64)) != 1 {
		t.Fatalf("unexpected debug payload: %#v", payload)
	}
	os.Unsetenv("OY_DEBUG")
	if got := DebugLogPath(); got != "" {
		t.Fatalf("expected empty debug path after disable, got %q", got)
	}
}

func TestResolvePathDeniesTraversal(t *testing.T) {
	root := "/tmp/work"
	if _, err := ResolvePath(root, "ok/file.txt"); err != nil {
		t.Fatalf("expected inside path, got error: %v", err)
	}
	if _, err := ResolvePath(root, "../etc/passwd"); err == nil {
		t.Fatal("expected traversal error")
	}
}

func mustReadFile(t *testing.T, path string) []byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func contains(text, needle string) bool { return strings.Contains(text, needle) }
