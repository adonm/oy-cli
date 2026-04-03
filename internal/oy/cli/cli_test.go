package cli

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/agent"
	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
)

func TestMainNormalizesCommands(t *testing.T) {
	oldRun, oldRalph := runCommand, ralphCommand
	defer func() { runCommand, ralphCommand = oldRun, oldRalph }()
	var runArgs, ralphArgs []string
	runCommand = func(args ...string) int {
		runArgs = append([]string(nil), args...)
		return 0
	}
	ralphCommand = func(args ...string) int {
		ralphArgs = append([]string(nil), args...)
		return 0
	}
	if code := Main([]string{"fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if code := Main([]string{"ralph", "fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if !reflect.DeepEqual(runArgs, []string{"fix", "tests"}) {
		t.Fatalf("unexpected run args: %#v", runArgs)
	}
	if !reflect.DeepEqual(ralphArgs, []string{"fix", "tests"}) {
		t.Fatalf("unexpected ralph args: %#v", ralphArgs)
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
	oldRunAgent, oldUnattended := runAgentFunc, unattendedLimitFunc
	defer func() { runAgentFunc, unattendedLimitFunc = oldRunAgent, oldUnattended }()
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		return 0, "", nil
	}
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
	oldRunAgent, oldUnattended := runAgentFunc, unattendedLimitFunc
	defer func() { runAgentFunc, unattendedLimitFunc = oldRunAgent, oldUnattended }()
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		return 0, "", nil
	}
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

func TestRunUsesAgentWithResolvedSession(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	t.Setenv("OY_BEST_OF", "3")
	oldRunAgent, oldUnattended := runAgentFunc, unattendedLimitFunc
	defer func() { runAgentFunc, unattendedLimitFunc = oldRunAgent, oldUnattended }()
	seen := map[string]any{}
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		seen = map[string]any{"prompt": prompt, "model": model, "workspace": workspace, "unattended": unattendedLimitSeconds, "interactive": interactive, "yolo": yolo, "transcript_nil": transcript == nil, "best_of": bestOf}
		return 7, "", nil
	}
	if code := Run("fix", "tests"); code != 7 {
		t.Fatalf("unexpected code: %d", code)
	}
	if seen["prompt"] != "fix tests" || seen["model"] != "openai:gpt-test" || seen["workspace"] != root || seen["unattended"] != 60 || seen["interactive"] != false || seen["yolo"] != true || seen["transcript_nil"] != true || seen["best_of"] != 3 {
		t.Fatalf("unexpected run agent args: %#v", seen)
	}
}

func TestRalphRunsPromptUntilDeadline(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	t.Setenv("OY_BEST_OF", "3")
	oldRunAgent, oldUnattended, oldLimit := runAgentFunc, unattendedLimitFunc, ralphLimitFunc
	oldNow, oldSleep := nowFunc, sleepFunc
	defer func() {
		runAgentFunc, unattendedLimitFunc, ralphLimitFunc = oldRunAgent, oldUnattended, oldLimit
		nowFunc, sleepFunc = oldNow, oldSleep
	}()
	calls := []map[string]any{}
	sleeps := []time.Duration{}
	times := []time.Time{
		time.Unix(0, 0),
		time.Unix(0, 0),
		time.Unix(60, 0),
		time.Unix(60, 0),
		time.Unix(120, 0),
	}
	index := 0
	nowFunc = func() time.Time {
		value := times[index]
		if index < len(times)-1 {
			index++
		}
		return value
	}
	sleepFunc = func(duration time.Duration) { sleeps = append(sleeps, duration) }
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	ralphLimitFunc = func() (int, error) { return 120, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		calls = append(calls, map[string]any{"prompt": prompt, "model": model, "workspace": workspace, "interactive": interactive, "yolo": yolo, "best_of": bestOf})
		return 0, "", nil
	}
	if code := Ralph("fix", "tests"); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if len(calls) != 2 {
		t.Fatalf("unexpected call count: %d %#v", len(calls), calls)
	}
	for _, call := range calls {
		if call["prompt"] != "fix tests" || call["model"] != "openai:gpt-test" || call["workspace"] != root || call["interactive"] != false || call["yolo"] != true || call["best_of"] != 3 {
			t.Fatalf("unexpected ralph call: %#v", call)
		}
	}
	if !reflect.DeepEqual(sleeps, []time.Duration{time.Minute}) {
		t.Fatalf("unexpected sleeps: %#v", sleeps)
	}
}

func TestAuditCreatesDefaultRenovateConfigAndRunsAgent(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	t.Setenv("OY_BEST_OF", "3")
	oldRunAgent, oldUnattended := runAgentFunc, unattendedLimitFunc
	defer func() { runAgentFunc, unattendedLimitFunc = oldRunAgent, oldUnattended }()
	seen := map[string]any{}
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		seen = map[string]any{"prompt": prompt, "model": model, "workspace": workspace, "system_prompt": systemPrompt, "unattended": unattendedLimitSeconds, "interactive": interactive, "yolo": yolo, "best_of": bestOf}
		return 0, "", nil
	}
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
	if seen["prompt"] != "Conduct a security and complexity audit of this repository. Additional focus: deps" || seen["model"] != "openai:gpt-test" || seen["workspace"] != root || seen["system_prompt"] != runtime.AuditSystemPrompt() || seen["unattended"] != 60 || seen["interactive"] != false || seen["yolo"] != false || seen["best_of"] != 3 {
		t.Fatalf("unexpected audit args: %#v", seen)
	}
}

func TestChatCommandTokensReportsPreparedBudget(t *testing.T) {
	tx := agent.TranscriptWithSystemPrompt("sys")
	agent.AddUser(&tx, "hello")
	var out strings.Builder
	oldStdout := stdoutWriter
	stdoutWriter = &out
	defer func() { stdoutWriter = oldStdout }()
	if result := ChatCommand("/tokens", &tx, "sys", "openai:gpt-test"); result != true {
		t.Fatalf("unexpected result: %#v", result)
	}
	text := out.String()
	if !strings.Contains(text, "prepared tokens") || !strings.Contains(text, "context budget") || !strings.Contains(text, "remaining") {
		t.Fatalf("unexpected tokens output: %q", text)
	}
}

func TestHandleLoadSupportsNumericSelectionByNewestFirst(t *testing.T) {
	SessionsDir = t.TempDir()
	defer func() { SessionsDir = "" }()
	writeSession := func(name, model string, delay time.Duration) {
		path := filepath.Join(SessionsDir, name+".json")
		payload := map[string]any{
			"model":      model,
			"transcript": TranscriptData(agent.TranscriptWithSystemPrompt("old")),
		}
		data, err := json.Marshal(payload)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		time.Sleep(delay)
	}
	writeSession("older", "openai:gpt-old", 10*time.Millisecond)
	writeSession("newer", "openai:gpt-new", 0)
	loaded, model, err := HandleLoad("1", agent.TranscriptWithSystemPrompt("sys"), "openai:gpt-test", "sys")
	if err != nil {
		t.Fatal(err)
	}
	if model != "openai:gpt-new" {
		t.Fatalf("unexpected model: %q", model)
	}
	if len(loaded.Messages) != 1 || loaded.Messages[0].Content != "sys" {
		t.Fatalf("unexpected transcript: %#v", loaded.Messages)
	}
}

func TestModelShowsShimAndCanFilterSwitch(t *testing.T) {
	oldList, oldStdout := listAllModelIDsFunc, stdoutWriter
	defer func() {
		listAllModelIDsFunc = oldList
		stdoutWriter = oldStdout
	}()
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1", "copilot:gpt-5"}, nil, nil
	}
	var out strings.Builder
	stdoutWriter = &out
	t.Setenv("OY_MODEL", "openai:gpt-5")
	if code := Model(""); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(out.String(), "- shim: `openai`") {
		t.Fatalf("missing shim in model output: %q", out.String())
	}
	out.Reset()
	if code := Model("gpt-4.1"); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(out.String(), "Default Model Updated") || !strings.Contains(out.String(), "openai:gpt-4.1") {
		t.Fatalf("unexpected switch output: %q", out.String())
	}
}

func TestChatListsSavedSessionsWhenLoadHasNoArg(t *testing.T) {
	SessionsDir = t.TempDir()
	defer func() { SessionsDir = "" }()
	payload := map[string]any{
		"model":      "openai:gpt-test",
		"saved_at":   "2026-03-25T12:34:56",
		"transcript": TranscriptData(agent.TranscriptWithSystemPrompt("sys")),
	}
	data, err := json.Marshal(payload)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(SessionsDir, "saved.json"), data, 0o644); err != nil {
		t.Fatal(err)
	}
	oldResolve, oldReader, oldStderr := resolveSessionFunc, chatInputReaderFunc, stderrWriter
	defer func() {
		resolveSessionFunc = oldResolve
		chatInputReaderFunc = oldReader
		stderrWriter = oldStderr
	}()
	resolveSessionFunc = func(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
		return runtime.Session(t.TempDir(), "openai:gpt-test", true, "sys", "", false, 1), nil
	}
	chatInputReaderFunc = func() io.Reader { return strings.NewReader("/load\n/quit\n") }
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Chat(); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(errOut.String(), "## Saved Sessions") || !strings.Contains(errOut.String(), "Usage: `/load <name>` or `/load <number>`") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestChatRollsBackOnAgentError(t *testing.T) {
	oldResolve, oldReader, oldRun, oldErr := resolveSessionFunc, chatInputReaderFunc, runAgentFunc, stderrWriter
	defer func() {
		resolveSessionFunc = oldResolve
		chatInputReaderFunc = oldReader
		runAgentFunc = oldRun
		stderrWriter = oldErr
	}()
	resolveSessionFunc = func(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
		return runtime.Session(t.TempDir(), "openai:gpt-test", true, "sys", "", false, 1), nil
	}
	chatInputReaderFunc = func() io.Reader { return strings.NewReader("hello\nquit\n") }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		return 1, "", fmt.Errorf("boom")
	}
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Chat(); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(errOut.String(), "Agent error: boom") {
		t.Fatalf("missing agent error: %q", errOut.String())
	}
}
