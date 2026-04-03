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
	"github.com/wagov-dtt/oy-cli/internal/oy/ui"
	"github.com/wagov-dtt/oy-cli/internal/oy/version"
)

type askStubClient struct{ t *testing.T }

func (s askStubClient) ChatCompletion(model string, _ []providers.ChatMessage, specs []map[string]any, _ string) (providers.ChatMessage, error) {
	if model != "gpt-test" {
		s.t.Fatalf("unexpected model: %q", model)
	}
	for _, spec := range specs {
		name, _ := spec["name"].(string)
		if name == "bash" || name == "replace" {
			s.t.Fatalf("unexpected write-capable tool in ask mode: %q", name)
		}
	}
	return providers.AssistantMessage("read-only answer", nil), nil
}

func (s askStubClient) ListModels() ([]string, error) { return nil, nil }

func TestMainNormalizesCommands(t *testing.T) {
	oldRun, oldRalph, oldAsk := runCommand, ralphCommand, askCommand
	defer func() { runCommand, ralphCommand, askCommand = oldRun, oldRalph, oldAsk }()
	var runArgs, ralphArgs, askArgs []string
	runCommand = func(args ...string) int {
		runArgs = append([]string(nil), args...)
		return 0
	}
	ralphCommand = func(args ...string) int {
		ralphArgs = append([]string(nil), args...)
		return 0
	}
	askCommand = func(args ...string) int {
		askArgs = append([]string(nil), args...)
		return 0
	}
	if code := Main([]string{"fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if code := Main([]string{"ralph", "fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if code := Main([]string{"ask", "what", "changed"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if !reflect.DeepEqual(runArgs, []string{"fix", "tests"}) {
		t.Fatalf("unexpected run args: %#v", runArgs)
	}
	if !reflect.DeepEqual(ralphArgs, []string{"fix", "tests"}) {
		t.Fatalf("unexpected ralph args: %#v", ralphArgs)
	}
	if !reflect.DeepEqual(askArgs, []string{"what", "changed"}) {
		t.Fatalf("unexpected ask args: %#v", askArgs)
	}
}

func TestMainRejectsTopLevelYolo(t *testing.T) {
	oldStderr := stderrWriter
	defer func() { stderrWriter = oldStderr }()
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Main([]string{"--yolo", "fix", "tests"}); code != 1 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if !strings.Contains(errOut.String(), "top-level --yolo is not allowed") || !strings.Contains(errOut.String(), "oy chat --yolo") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestMainSupportsChatYoloFlag(t *testing.T) {
	oldChat := chatCommand
	defer func() { chatCommand = oldChat }()
	chatCommand = func() int {
		if os.Getenv("OY_YOLO") != "1" {
			t.Fatalf("expected OY_YOLO during chat command, got %q", os.Getenv("OY_YOLO"))
		}
		return 0
	}
	os.Unsetenv("OY_YOLO")
	if code := Main([]string{"chat", "--yolo"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if value := os.Getenv("OY_YOLO"); value != "" {
		t.Fatalf("expected OY_YOLO restored after chat command, got %q", value)
	}
}

func TestMainHelpMatchesBaselineStyleOutput(t *testing.T) {
	oldStdout := stdoutWriter
	defer func() { stdoutWriter = oldStdout }()
	var out strings.Builder
	stdoutWriter = &out
	if code := Main([]string{"--help"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	text := out.String()
	for _, needle := range []string{
		"usage: oy [-h] [--version] [--model MODEL] [--root DIR] [--system-file FILE] [--best-of N] [--non-interactive] {run,chat,ralph,ask,model,audit,help} ...",
		"AI coding assistant for your shell.",
		"positional arguments:",
		"run                  Run a one-shot task.",
		"chat                 Start an interactive multi-turn chat session.",
		"ask                  Run a one-shot research-only query.",
		"help                 Show top-level or command-specific help.",
		"options:",
		"--version             show program's version number and exit",
		"--model MODEL         override model for this command",
		"--root DIR            run against a different workspace",
		"--system-file FILE    append extra system instructions",
		"--best-of N           override self-consistency count",
		"--non-interactive     disable prompt/approval pauses",
		"Examples:",
		"oy --model openai:gpt-5 --best-of 1 \"fix the flaky test\"",
		"oy --root ../service audit auth",
		"oy --system-file .oy/system.md chat",
		"oy chat --yolo",
		"oy model list",
	} {
		if !strings.Contains(text, needle) {
			t.Fatalf("missing help text %q in %q", needle, text)
		}
	}
}

func TestMainVersionUsesInjectedVersion(t *testing.T) {
	oldStdout, oldVersion := stdoutWriter, version.Version
	defer func() {
		stdoutWriter = oldStdout
		version.Version = oldVersion
	}()
	var out strings.Builder
	stdoutWriter = &out
	version.Version = "1.2.3"
	if code := Main([]string{"--version"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if out.String() != "oy 1.2.3\n" {
		t.Fatalf("unexpected version output: %q", out.String())
	}
}

func TestChatHelpMatchesBaselineStyleOutput(t *testing.T) {
	oldStdout := stdoutWriter
	defer func() { stdoutWriter = oldStdout }()
	var out strings.Builder
	stdoutWriter = &out
	if code := Main([]string{"chat", "--help"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	text := out.String()
	for _, needle := range []string{
		"usage: oy chat [-h] [--yolo]",
		"Start an interactive multi-turn chat session.",
		"options:",
		"-h, --help  show this help message and exit",
		"--yolo      Allow all tools without per-action approval prompts.",
		"In chat, use /help to see slash commands.",
	} {
		if !strings.Contains(text, needle) {
			t.Fatalf("missing chat help text %q in %q", needle, text)
		}
	}
}

func TestMainHelpTopicPrintsCommandHelp(t *testing.T) {
	oldStdout := stdoutWriter
	defer func() { stdoutWriter = oldStdout }()
	var out strings.Builder
	stdoutWriter = &out
	if code := Main([]string{"help", "model"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	text := out.String()
	for _, needle := range []string{
		"usage: oy model [selection|list]",
		"Show, list, or change the default model.",
		"oy model list",
	} {
		if !strings.Contains(text, needle) {
			t.Fatalf("missing model help text %q in %q", needle, text)
		}
	}
}

func TestMainAskHelpTopicPrintsCommandHelp(t *testing.T) {
	oldStdout := stdoutWriter
	defer func() { stdoutWriter = oldStdout }()
	var out strings.Builder
	stdoutWriter = &out
	if code := Main([]string{"help", "ask"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	text := out.String()
	for _, needle := range []string{
		"usage: oy ask <question>",
		"Run a one-shot research-only query.",
		"No bash or file changes; public webfetch is still allowed.",
	} {
		if !strings.Contains(text, needle) {
			t.Fatalf("missing ask help text %q in %q", needle, text)
		}
	}
}

func TestMainAppliesGlobalFlagsBeforeImplicitRun(t *testing.T) {
	oldRun := runCommand
	defer func() { runCommand = oldRun }()
	var seen map[string]string
	runCommand = func(args ...string) int {
		seen = map[string]string{
			"arg0":            args[0],
			"arg1":            args[1],
			"model":           os.Getenv("OY_MODEL"),
			"root":            os.Getenv("OY_ROOT"),
			"system_file":     os.Getenv("OY_SYSTEM_FILE"),
			"best_of":         os.Getenv("OY_BEST_OF"),
			"non_interactive": os.Getenv("OY_NON_INTERACTIVE"),
		}
		return 0
	}
	os.Unsetenv("OY_MODEL")
	os.Unsetenv("OY_ROOT")
	os.Unsetenv("OY_SYSTEM_FILE")
	os.Unsetenv("OY_BEST_OF")
	os.Unsetenv("OY_NON_INTERACTIVE")
	if code := Main([]string{"--model", "openai:gpt-test", "--root", "/tmp/work", "--system-file", "extra.md", "--best-of", "5", "--non-interactive", "fix", "tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if seen == nil {
		t.Fatal("expected run command to be called")
	}
	if seen["arg0"] != "fix" || seen["arg1"] != "tests" || seen["model"] != "openai:gpt-test" || seen["root"] != "/tmp/work" || seen["system_file"] != "extra.md" || seen["best_of"] != "5" || seen["non_interactive"] != "1" {
		t.Fatalf("unexpected global flag state: %#v", seen)
	}
	if os.Getenv("OY_MODEL") != "" || os.Getenv("OY_ROOT") != "" || os.Getenv("OY_SYSTEM_FILE") != "" || os.Getenv("OY_BEST_OF") != "" || os.Getenv("OY_NON_INTERACTIVE") != "" {
		t.Fatalf("expected global flags to restore environment")
	}
}

func TestMainRestoresExistingEnvAfterGlobalFlags(t *testing.T) {
	oldRun := runCommand
	defer func() { runCommand = oldRun }()
	runCommand = func(args ...string) int {
		if os.Getenv("OY_MODEL") != "openai:gpt-override" || os.Getenv("OY_ROOT") != "/tmp/override" {
			t.Fatalf("expected overrides during command, got model=%q root=%q", os.Getenv("OY_MODEL"), os.Getenv("OY_ROOT"))
		}
		return 0
	}
	os.Setenv("OY_MODEL", "openai:gpt-saved")
	os.Setenv("OY_ROOT", "/tmp/original")
	defer os.Unsetenv("OY_MODEL")
	defer os.Unsetenv("OY_ROOT")
	if code := Main([]string{"--model=openai:gpt-override", "--root=/tmp/override", "run", "fix tests"}); code != 0 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if os.Getenv("OY_MODEL") != "openai:gpt-saved" || os.Getenv("OY_ROOT") != "/tmp/original" {
		t.Fatalf("expected original env restored, got model=%q root=%q", os.Getenv("OY_MODEL"), os.Getenv("OY_ROOT"))
	}
}

func TestMainRejectsUnknownTopLevelOption(t *testing.T) {
	oldStderr := stderrWriter
	defer func() { stderrWriter = oldStderr }()
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Main([]string{"--wat"}); code != 1 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if !strings.Contains(errOut.String(), "unknown top-level option: --wat") || !strings.Contains(errOut.String(), "oy --help") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestMainRejectsMissingGlobalOptionValue(t *testing.T) {
	oldStderr := stderrWriter
	defer func() { stderrWriter = oldStderr }()
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Main([]string{"--model"}); code != 1 {
		t.Fatalf("unexpected exit code: %d", code)
	}
	if !strings.Contains(errOut.String(), "missing value for --model") || !strings.Contains(errOut.String(), "oy --help") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
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

func TestHandleModelSwitchAcceptsLsAlias(t *testing.T) {
	oldList := listAllModelIDsFunc
	defer func() { listAllModelIDsFunc = oldList }()
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1"}, nil, nil
	}
	var out strings.Builder
	current := handleModelSwitch("ls", "openai:gpt-5", ".", &out)
	if current != "openai:gpt-5" {
		t.Fatalf("unexpected current model: %q", current)
	}
	if !strings.Contains(out.String(), "## Available Models") || !strings.Contains(out.String(), "openai:gpt-4.1") {
		t.Fatalf("unexpected output: %q", out.String())
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

func TestAgentNoteFuncWritesToStderrWriter(t *testing.T) {
	oldStderr := stderrWriter
	defer func() { stderrWriter = oldStderr }()
	var errOut strings.Builder
	stderrWriter = &errOut
	agent.NoteFunc("waiting for llm: openai:gpt-test (turn 1)")
	if errOut.String() != "[note] waiting for llm: openai:gpt-test (turn 1)\n" {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestRunPrintsAgentError(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	oldRunAgent, oldUnattended, oldStderr := runAgentFunc, unattendedLimitFunc, stderrWriter
	defer func() { runAgentFunc, unattendedLimitFunc, stderrWriter = oldRunAgent, oldUnattended, oldStderr }()
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		return 1, "", fmt.Errorf("boom")
	}
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Run("fix", "tests"); code != 1 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(errOut.String(), "Agent error: boom") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestAskRunsReadOnlyResearch(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	t.Setenv("OY_BEST_OF", "3")
	oldReq, oldClient, oldUnattended, oldStdout, oldStderr, oldPrint := requireAPIEnvFunc, getClientFunc, unattendedLimitFunc, stdoutWriter, stderrWriter, agent.PrintFunc
	defer func() {
		requireAPIEnvFunc, getClientFunc, unattendedLimitFunc = oldReq, oldClient, oldUnattended
		stdoutWriter, stderrWriter, agent.PrintFunc = oldStdout, oldStderr, oldPrint
	}()
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	requireAPIEnvFunc = func(model, _, workspace string) (string, error) {
		if model != "openai:gpt-test" || workspace != root {
			t.Fatalf("unexpected require args: model=%q workspace=%q", model, workspace)
		}
		return "openai", nil
	}
	getClientFunc = func(shim, workspace string) (providers.CompletionClient, error) {
		if shim != "openai" || workspace != root {
			t.Fatalf("unexpected client args: shim=%q workspace=%q", shim, workspace)
		}
		return askStubClient{t: t}, nil
	}
	var out, errOut strings.Builder
	stdoutWriter = &out
	stderrWriter = &errOut
	agent.PrintFunc = func(value string) { fmt.Fprintln(stdoutWriter, value) }
	if code := Ask("summarize", "auth"); code != 0 {
		t.Fatalf("unexpected code: %d stdout=%q stderr=%q", code, out.String(), errOut.String())
	}
	if !strings.Contains(errOut.String(), "## Ask") || !strings.Contains(errOut.String(), AskModeNote) {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
	if !strings.Contains(out.String(), "read-only answer") {
		t.Fatalf("unexpected stdout: %q", out.String())
	}
}

func TestRalphRunsPromptUntilDeadline(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	t.Setenv("OY_BEST_OF", "3")
	oldRunAgent, oldUnattended, oldLimit := runAgentFunc, unattendedLimitFunc, ralphLimitFunc
	oldNow, oldSleep, oldErr, oldPrompt := nowFunc, sleepFunc, stderrWriter, newPromptIOFunc
	defer func() {
		runAgentFunc, unattendedLimitFunc, ralphLimitFunc = oldRunAgent, oldUnattended, oldLimit
		nowFunc, sleepFunc = oldNow, oldSleep
		stderrWriter = oldErr
		newPromptIOFunc = oldPrompt
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
	var errOut strings.Builder
	stderrWriter = &errOut
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
	if !strings.Contains(errOut.String(), "## Ralph") || !strings.Contains(errOut.String(), "- schedule: `until 2m deadline, 1m delay`") || !strings.Contains(errOut.String(), "[note] ralph run 1 (~2m remaining)") || !strings.Contains(errOut.String(), "[note] ralph run 2 (~1m remaining)") {
		t.Fatalf("unexpected ralph stderr: %q", errOut.String())
	}
}

func TestAuditCreatesDefaultRenovateConfigAndRunsAgent(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	t.Setenv("OY_BEST_OF", "3")
	oldRunAgent, oldUnattended, oldErr := runAgentFunc, unattendedLimitFunc, stderrWriter
	defer func() { runAgentFunc, unattendedLimitFunc = oldRunAgent, oldUnattended; stderrWriter = oldErr }()
	seen := map[string]any{}
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		seen = map[string]any{"prompt": prompt, "model": model, "workspace": workspace, "system_prompt": systemPrompt, "unattended": unattendedLimitSeconds, "interactive": interactive, "yolo": yolo, "best_of": bestOf}
		return 0, "", nil
	}
	var errOut strings.Builder
	stderrWriter = &errOut
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
	if !strings.Contains(errOut.String(), "[note] created default Renovate config: renovate.json") || !strings.Contains(errOut.String(), "## Audit") || !strings.Contains(errOut.String(), "- focus: `deps`") || !strings.Contains(errOut.String(), "[note] audit mode") {
		t.Fatalf("unexpected audit stderr: %q", errOut.String())
	}
}

func TestAuditPrintsAgentError(t *testing.T) {
	root := t.TempDir()
	t.Setenv("OY_ROOT", root)
	t.Setenv("OY_MODEL", "openai:gpt-test")
	oldRunAgent, oldUnattended, oldStderr := runAgentFunc, unattendedLimitFunc, stderrWriter
	defer func() { runAgentFunc, unattendedLimitFunc, stderrWriter = oldRunAgent, oldUnattended, oldStderr }()
	unattendedLimitFunc = func() (int, error) { return 60, nil }
	runAgentFunc = func(prompt, model, workspace, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
		return 1, "", fmt.Errorf("boom")
	}
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Audit("deps"); code != 1 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(errOut.String(), "Audit error: boom") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
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
	oldList, oldStdout, oldCanPrompt := listAllModelIDsFunc, stdoutWriter, canPromptFunc
	defer func() {
		listAllModelIDsFunc = oldList
		stdoutWriter = oldStdout
		canPromptFunc = oldCanPrompt
	}()
	canPromptFunc = func() bool { return false }
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1", "copilot:gpt-5"}, nil, nil
	}
	var out strings.Builder
	stdoutWriter = &out
	t.Setenv("OY_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	t.Setenv("OY_MODEL", "openai:gpt-5")
	t.Setenv("OY_SHIM", "")
	if code := Model(""); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(out.String(), "- shim: `openai`") || !strings.Contains(out.String(), "oy model list") {
		t.Fatalf("missing current-model hints in output: %q", out.String())
	}
	out.Reset()
	if code := Model("list"); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(out.String(), "## Available Models") || !strings.Contains(out.String(), "openai:gpt-4.1") || !strings.Contains(out.String(), "copilot:gpt-5") {
		t.Fatalf("unexpected list output: %q", out.String())
	}
	out.Reset()
	if code := Model("openai:gpt-4.1"); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(out.String(), "Default Model Updated") || !strings.Contains(out.String(), "openai:gpt-4.1") {
		t.Fatalf("unexpected switch output: %q", out.String())
	}
}

func TestModelInteractivePickerSavesSelection(t *testing.T) {
	oldList, oldStdout, oldStderr := listAllModelIDsFunc, stdoutWriter, stderrWriter
	oldCanPrompt, oldInput, oldPrompt := canPromptFunc, modelInputReaderFunc, newPromptIOFunc
	defer func() {
		listAllModelIDsFunc = oldList
		stdoutWriter = oldStdout
		stderrWriter = oldStderr
		canPromptFunc = oldCanPrompt
		modelInputReaderFunc = oldInput
		newPromptIOFunc = oldPrompt
	}()
	canPromptFunc = func() bool { return true }
	modelInputReaderFunc = func() io.Reader { return strings.NewReader("y\n2\n") }
	newPromptIOFunc = func(input io.Reader, output io.Writer) promptIO { return ui.NewPromptIO(input, output, false) }
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1", "copilot:gpt-5"}, nil, nil
	}
	workspace := t.TempDir()
	t.Setenv("OY_ROOT", workspace)
	t.Setenv("OY_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	t.Setenv("OY_MODEL", "openai:gpt-5")
	t.Setenv("OY_SHIM", "")
	var out, errOut strings.Builder
	stdoutWriter = &out
	stderrWriter = &errOut
	if code := Model(""); code != 0 {
		t.Fatalf("unexpected code: %d stderr=%q", code, errOut.String())
	}
	if !strings.Contains(out.String(), "Default Model Updated") || !strings.Contains(out.String(), "openai:gpt-4.1") {
		t.Fatalf("unexpected model output: %q", out.String())
	}
	if !strings.Contains(errOut.String(), "Pick a new model?") || !strings.Contains(errOut.String(), "## Choose a Model") {
		t.Fatalf("unexpected interactive output: %q", errOut.String())
	}
	if got := runtime.LoadModelConfig(); got.Model != "gpt-4.1" || got.Shim != "openai" {
		t.Fatalf("unexpected saved config: %#v", got)
	}
}

func TestModelNonInteractiveRequiresExactMatch(t *testing.T) {
	oldList, oldStderr, oldCanPrompt := listAllModelIDsFunc, stderrWriter, canPromptFunc
	defer func() {
		listAllModelIDsFunc = oldList
		stderrWriter = oldStderr
		canPromptFunc = oldCanPrompt
	}()
	canPromptFunc = func() bool { return false }
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1", "copilot:gpt-5"}, nil, nil
	}
	t.Setenv("OY_MODEL", "openai:gpt-5")
	t.Setenv("OY_SHIM", "")
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Model("gpt-5"); code != 1 {
		t.Fatalf("unexpected code: %d stderr=%q", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "## Matching Models") || !strings.Contains(errOut.String(), "No exact model match") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestModelNonInteractiveAcceptsUniqueSubstringMatch(t *testing.T) {
	oldList, oldStdout, oldStderr, oldCanPrompt := listAllModelIDsFunc, stdoutWriter, stderrWriter, canPromptFunc
	defer func() {
		listAllModelIDsFunc = oldList
		stdoutWriter = oldStdout
		stderrWriter = oldStderr
		canPromptFunc = oldCanPrompt
	}()
	canPromptFunc = func() bool { return false }
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1", "copilot:gpt-5"}, nil, nil
	}
	t.Setenv("OY_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	var out, errOut strings.Builder
	stdoutWriter = &out
	stderrWriter = &errOut
	if code := Model("4.1"); code != 0 {
		t.Fatalf("unexpected code: %d stdout=%q stderr=%q", code, out.String(), errOut.String())
	}
	if !strings.Contains(out.String(), "openai:gpt-4.1") {
		t.Fatalf("unexpected stdout: %q", out.String())
	}
	if errOut.Len() != 0 {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestModelListAcceptsLsAlias(t *testing.T) {
	oldList, oldStdout, oldCanPrompt := listAllModelIDsFunc, stdoutWriter, canPromptFunc
	defer func() {
		listAllModelIDsFunc = oldList
		stdoutWriter = oldStdout
		canPromptFunc = oldCanPrompt
	}()
	canPromptFunc = func() bool { return false }
	listAllModelIDsFunc = func(string) ([]string, []string, error) {
		return []string{"openai:gpt-5", "openai:gpt-4.1"}, nil, nil
	}
	var out strings.Builder
	stdoutWriter = &out
	if code := Model("ls"); code != 0 {
		t.Fatalf("unexpected code: %d stdout=%q", code, out.String())
	}
	if !strings.Contains(out.String(), "## Available Models") || !strings.Contains(out.String(), "openai:gpt-4.1") {
		t.Fatalf("unexpected stdout: %q", out.String())
	}
}

func TestModelWithoutConfigShowsActionableHint(t *testing.T) {
	oldStderr, oldCanPrompt := stderrWriter, canPromptFunc
	defer func() {
		stderrWriter = oldStderr
		canPromptFunc = oldCanPrompt
	}()
	canPromptFunc = func() bool { return false }
	t.Setenv("OY_MODEL", "")
	t.Setenv("OY_SHIM", "")
	t.Setenv("OY_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Model(""); code != 1 {
		t.Fatalf("unexpected code: %d stderr=%q", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "no model configured") || !strings.Contains(errOut.String(), "oy model list") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestRunWithoutPromptPrintsHelpInsteadOfEnteringChat(t *testing.T) {
	oldStdout, oldStderr, oldHasTTY, oldReadStdin, oldChat := stdoutWriter, stderrWriter, hasTTYStdinFunc, readStdinFunc, chatCommand
	defer func() {
		stdoutWriter = oldStdout
		stderrWriter = oldStderr
		hasTTYStdinFunc = oldHasTTY
		readStdinFunc = oldReadStdin
		chatCommand = oldChat
	}()
	hasTTYStdinFunc = func() bool { return true }
	readStdinFunc = func() string { return "" }
	chatCalled := false
	chatCommand = func() int {
		chatCalled = true
		return 0
	}
	var out, errOut strings.Builder
	stdoutWriter = &out
	stderrWriter = &errOut
	if code := Run(); code != 1 {
		t.Fatalf("unexpected code: %d stdout=%q stderr=%q", code, out.String(), errOut.String())
	}
	if chatCalled {
		t.Fatal("expected chat command not to be called")
	}
	if !strings.Contains(out.String(), "usage: oy run [prompt]") || !strings.Contains(out.String(), "oy chat") {
		t.Fatalf("unexpected stdout: %q", out.String())
	}
	if !strings.Contains(errOut.String(), "use `oy chat` for interactive multi-turn mode") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestRunChatUnknownOptionSuggestsHelp(t *testing.T) {
	oldStderr := stderrWriter
	defer func() { stderrWriter = oldStderr }()
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Main([]string{"chat", "--wat"}); code != 1 {
		t.Fatalf("unexpected code: %d stderr=%q", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "unknown chat option") || !strings.Contains(errOut.String(), "oy chat --help") {
		t.Fatalf("unexpected stderr: %q", errOut.String())
	}
}

func TestHandleDebugToggleTogglesDebugLog(t *testing.T) {
	t.Setenv("OY_CONFIG", filepath.Join(t.TempDir(), "config.json"))
	oldStderr := stderrWriter
	defer func() {
		stderrWriter = oldStderr
		os.Unsetenv("OY_DEBUG")
		_ = runtime.DisableDebugLog()
	}()
	var errOut strings.Builder
	stderrWriter = &errOut
	os.Unsetenv("OY_DEBUG")
	if err := runtime.DisableDebugLog(); err != nil {
		t.Fatal(err)
	}
	handleDebugToggle()
	first := errOut.String()
	if !strings.Contains(first, "debug logging enabled:") || runtime.DebugLogPath() == "" {
		t.Fatalf("unexpected enable output/path: %q %q", first, runtime.DebugLogPath())
	}
	errOut.Reset()
	handleDebugToggle()
	if !strings.Contains(errOut.String(), "debug logging disabled") || runtime.DebugLogPath() != "" {
		t.Fatalf("unexpected disable output/path: %q %q", errOut.String(), runtime.DebugLogPath())
	}
}

func TestChatShowsGitDiffSummaryBeforePrompt(t *testing.T) {
	oldResolve, oldReader, oldStderr, oldGitDiff, oldPrompt := resolveSessionFunc, chatInputReaderFunc, stderrWriter, gitDiffShortstatFunc, newPromptIOFunc
	defer func() {
		resolveSessionFunc = oldResolve
		chatInputReaderFunc = oldReader
		stderrWriter = oldStderr
		gitDiffShortstatFunc = oldGitDiff
		newPromptIOFunc = oldPrompt
	}()
	resolveSessionFunc = func(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
		return runtime.Session(t.TempDir(), "openai:gpt-test", true, "sys", "", false, 1), nil
	}
	chatInputReaderFunc = func() io.Reader { return strings.NewReader("/quit\n") }
	newPromptIOFunc = func(input io.Reader, output io.Writer) promptIO { return ui.NewPromptIO(input, output, false) }
	gitDiffShortstatFunc = func(string) string { return "git diff: clean" }
	var errOut strings.Builder
	stderrWriter = &errOut
	if code := Chat(); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if !strings.Contains(errOut.String(), "git diff: clean") || !strings.Contains(errOut.String(), "oy >") {
		t.Fatalf("missing git diff prompt summary: %q", errOut.String())
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
	oldResolve, oldReader, oldStderr, oldPrompt := resolveSessionFunc, chatInputReaderFunc, stderrWriter, newPromptIOFunc
	defer func() {
		resolveSessionFunc = oldResolve
		chatInputReaderFunc = oldReader
		stderrWriter = oldStderr
		newPromptIOFunc = oldPrompt
	}()
	resolveSessionFunc = func(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
		return runtime.Session(t.TempDir(), "openai:gpt-test", true, "sys", "", false, 1), nil
	}
	chatInputReaderFunc = func() io.Reader { return strings.NewReader("/load\n/quit\n") }
	newPromptIOFunc = func(input io.Reader, output io.Writer) promptIO { return ui.NewPromptIO(input, output, false) }
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
	oldResolve, oldReader, oldRun, oldErr, oldPrompt := resolveSessionFunc, chatInputReaderFunc, runAgentFunc, stderrWriter, newPromptIOFunc
	defer func() {
		resolveSessionFunc = oldResolve
		chatInputReaderFunc = oldReader
		runAgentFunc = oldRun
		stderrWriter = oldErr
		newPromptIOFunc = oldPrompt
	}()
	resolveSessionFunc = func(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
		return runtime.Session(t.TempDir(), "openai:gpt-test", true, "sys", "", false, 1), nil
	}
	chatInputReaderFunc = func() io.Reader { return strings.NewReader("hello\nquit\n") }
	newPromptIOFunc = func(input io.Reader, output io.Writer) promptIO { return ui.NewPromptIO(input, output, false) }
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

type closeTrackingPrompt struct {
	ui.PromptIO
	closeCount *int
}

func (p closeTrackingPrompt) Close() error {
	if p.closeCount != nil {
		*p.closeCount = *p.closeCount + 1
	}
	return nil
}

func TestChatClosesPromptIOOnExit(t *testing.T) {
	oldResolve, oldReader, oldErr, oldPrompt := resolveSessionFunc, chatInputReaderFunc, stderrWriter, newPromptIOFunc
	defer func() {
		resolveSessionFunc = oldResolve
		chatInputReaderFunc = oldReader
		stderrWriter = oldErr
		newPromptIOFunc = oldPrompt
	}()
	resolveSessionFunc = func(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
		return runtime.Session(t.TempDir(), "openai:gpt-test", true, "sys", "", false, 1), nil
	}
	chatInputReaderFunc = func() io.Reader { return strings.NewReader("quit\n") }
	closed := 0
	newPromptIOFunc = func(input io.Reader, output io.Writer) promptIO {
		return closeTrackingPrompt{PromptIO: ui.NewPromptIO(input, output, false), closeCount: &closed}
	}
	stderrWriter = io.Discard
	if code := Chat(); code != 0 {
		t.Fatalf("unexpected code: %d", code)
	}
	if closed != 1 {
		t.Fatalf("expected prompt close once, got %d", closed)
	}
}
