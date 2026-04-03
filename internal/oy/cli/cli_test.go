package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/wagov-dtt/oy-cli/internal/oy/agent"
	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

func TestMainNormalizesCommands(t *testing.T) {
	if code := Main([]string{"fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if code := Main([]string{"ralph", "fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
}

func TestMainRejectsTopLevelYolo(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Fatal("expected panic for top-level --yolo")
		}
	}()
	_ = Main([]string{"--yolo", "fix", "tests"})
}

func TestAuditCreatesDefaultRenovateConfigWhenMissing(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	if code := Audit("deps"); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	content, err := os.ReadFile(filepath.Join(root, "renovate.json"))
	if err != nil {
		t.Fatal(err)
	}
	if string(content) != DefaultRenovateConfig {
		t.Fatalf("unexpected renovate config: %q", string(content))
	}
}

func TestAuditKeepsExistingSupportedRenovateConfig(t *testing.T) {
	root := t.TempDir()
	configDir := filepath.Join(root, ".github")
	if err := os.MkdirAll(configDir, 0o755); err != nil {
		t.Fatal(err)
	}
	existing := filepath.Join(configDir, "renovate.json")
	if err := os.WriteFile(existing, []byte("{\"extends\": [\"local>example/preset\"]}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	if code := Audit(""); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if _, err := os.Stat(filepath.Join(root, "renovate.json")); !os.IsNotExist(err) {
		t.Fatalf("unexpected generated renovate.json: %v", err)
	}
}

func TestHelpListsChatCommands(t *testing.T) {
	tx := agent.TranscriptState(nil, 100, 100)
	result := ChatCommand("/help", &tx, "sys", "openai:gpt-test")
	if result != true {
		t.Fatalf("unexpected result: %#v", result)
	}
}

func TestLoadAndChatCommands(t *testing.T) {
	SessionsDir = t.TempDir()
	defer func() { SessionsDir = "" }()
	saved := map[string]any{
		"model":    "openai:gpt-test",
		"saved_at": "2026-03-25T12:34:56",
		"transcript": TranscriptData(agent.TranscriptState([]providers.ChatMessage{
			providers.SystemMessage("old"),
			providers.UserMessage("hello"),
		}, 100, 100)),
	}
	data, err := json.Marshal(saved)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(SessionsDir, "saved.json"), data, 0o644); err != nil {
		t.Fatal(err)
	}
	loaded, model, err := HandleLoad("saved", agent.TranscriptWithSystemPrompt("sys"), "openai:gpt-old", "new system")
	if err != nil {
		t.Fatal(err)
	}
	if model != "openai:gpt-test" {
		t.Fatalf("unexpected model: %q", model)
	}
	if len(loaded.Messages) != 2 || loaded.Messages[0].Content != "new system" || loaded.Messages[1].Content != "hello" {
		t.Fatalf("unexpected loaded transcript: %#v", loaded.Messages)
	}
	if got := ChatCommand("/yolo", &loaded, "new system", model).([]string)[0]; got != "yolo" {
		t.Fatalf("unexpected yolo command: %q", got)
	}
	if !strings.HasSuffix(SessionFile("bad name/.."), "bad_name___.json") {
		t.Fatalf("unexpected session file path: %q", SessionFile("bad name/.."))
	}
	if result := ChatCommand("/clear", &loaded, "new system", model); result != true {
		t.Fatalf("unexpected clear result: %#v", result)
	}
	if len(loaded.Messages) != 1 || loaded.Messages[0].Content != "new system" {
		t.Fatalf("unexpected cleared transcript: %#v", loaded.Messages)
	}
}
