package cli

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/agent"
	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
	"github.com/wagov-dtt/oy-cli/internal/oy/version"
)

var SessionsDir string

var (
	runCommand         = Run
	chatCommand        = Chat
	ralphCommand       = Ralph
	auditCommand       = Audit
	modelCommand       = Model
	resolveSessionFunc = ResolveSession
	runAgentFunc       = defaultRunAgent
	hasTTYStdinFunc    = runtime.HasTTYStdin
	readStdinFunc      = func() string {
		data, _ := io.ReadAll(os.Stdin)
		return strings.TrimSpace(string(data))
	}
	unattendedLimitFunc = runtime.UnattendedLimitSeconds
	ralphLimitFunc      = runtime.RalphLimitSeconds
	nowFunc             = time.Now
	sleepFunc           = time.Sleep
)

const (
	AskRules              = "no bash or file changes; public webfetch still allowed"
	AskUsage              = "Usage: `/ask <question>` — research the codebase with " + AskRules + "."
	AskModeNote           = "research mode (" + AskRules + ")"
	DefaultRenovateConfig = "{\n  \"extends\": [\"config:recommended\", \"helpers:pinGitHubActionDigests\"]\n}\n"
)

var RenovateConfigCandidates = []string{
	"renovate.json",
	"renovate.json5",
	".github/renovate.json",
	".github/renovate.json5",
	".gitlab/renovate.json",
	".gitlab/renovate.json5",
	".renovaterc",
	".renovaterc.json",
	".renovaterc.json5",
}

var ChatCommandHelp = [][2]string{
	{"/help", "show this help"},
	{"/tokens", "show context usage"},
	{"/model [filter]", "show or switch model"},
	{"/debug", "toggle debug logging"},
	{"/yolo", "allow all tools for the rest of this session"},
	{"/ask <question>", "research-only query (" + AskRules + ")"},
	{"/audit [focus]", "run a security/complexity audit"},
	{"/save [name]", "save session transcript"},
	{"/load [name]", "load a saved session"},
	{"/undo", "remove the last prompt and its follow-up messages"},
	{"/clear", "reset conversation (keeps system prompt)"},
	{"/quit", "end session"},
	{"/exit", "end session"},
}

func Main(argv []string) int {
	args := append([]string(nil), argv...)
	commands := map[string]struct{}{"run": {}, "chat": {}, "ralph": {}, "model": {}, "audit": {}, "-h": {}, "--help": {}}
	if len(args) == 0 {
		if runtime.StdinIsInteractive() {
			PrintHelp()
			return 0
		}
		args = []string{"run"}
	} else if args[0] == "-v" || args[0] == "--version" {
		fmt.Printf("oy %s\n", version.Version)
		return 0
	} else if args[0] == "--yolo" {
		panic("top-level --yolo is not allowed; put it after a subcommand")
	} else if !strings.HasPrefix(args[0], "-") {
		if _, ok := commands[args[0]]; !ok {
			args = append([]string{"run"}, args...)
		}
	}
	if len(args) == 0 {
		PrintHelp()
		return 0
	}
	switch args[0] {
	case "run":
		return runCommand(args[1:]...)
	case "chat":
		return chatCommand()
	case "ralph":
		return ralphCommand(args[1:]...)
	case "audit":
		focus := ""
		if len(args) > 1 {
			focus = strings.Join(args[1:], " ")
		}
		return auditCommand(focus)
	case "model":
		selection := ""
		if len(args) > 1 {
			selection = strings.Join(args[1:], " ")
		}
		return modelCommand(selection)
	case "-h", "--help":
		PrintHelp()
		return 0
	default:
		PrintHelp()
		return 0
	}
}

func PrintHelp() {
	_, _ = fmt.Fprintln(os.Stdout, "oy")
	_, _ = fmt.Fprintln(os.Stdout, "")
	_, _ = fmt.Fprintln(os.Stdout, "Commands: run, chat, ralph, model, audit")
	_, _ = fmt.Fprintln(os.Stdout, "Progress is tracked in GO_PORT_TRACKER.md")
}

func WorkspaceRoot() (string, error) {
	workspace := strings.TrimSpace(os.Getenv("OY_ROOT"))
	if workspace == "" {
		workspace = "."
	}
	resolved, err := filepath.Abs(workspace)
	if err != nil {
		return "", err
	}
	info, err := os.Stat(resolved)
	if err != nil || !info.IsDir() {
		return "", fmt.Errorf("workspace root is not a directory: %s", resolved)
	}
	return resolved, nil
}

func LoadSystemPrompt(systemFile string, interactive bool) (string, error) {
	base := runtime.ActiveSystemPrompt(interactive)
	if strings.TrimSpace(systemFile) == "" {
		return base, nil
	}
	data, err := os.ReadFile(systemFile)
	if err != nil {
		return "", err
	}
	return base + "\n\n" + string(data), nil
}

func ResolveSession(interactive *bool, systemPrompt string, includeSystemFile bool, bestOf *int) (runtime.SessionContext, error) {
	resolvedInteractive := runtime.CanPrompt()
	if interactive != nil {
		resolvedInteractive = *interactive
	}
	workspace, err := WorkspaceRoot()
	if err != nil {
		return runtime.SessionContext{}, err
	}
	modelSpec, err := runtime.CurrentModel("")
	if err != nil {
		return runtime.SessionContext{}, err
	}
	systemFile := ""
	if includeSystemFile {
		systemFile = strings.TrimSpace(os.Getenv("OY_SYSTEM_FILE"))
	}
	resolvedSystemPrompt := systemPrompt
	if resolvedSystemPrompt == "" {
		resolvedSystemPrompt, err = LoadSystemPrompt(systemFile, resolvedInteractive)
		if err != nil {
			return runtime.SessionContext{}, err
		}
	}
	best := 0
	if bestOf != nil {
		best = *bestOf
	}
	resolvedBestOf, err := runtime.SelfConsistencyBestOf(best, modelSpec)
	if err != nil {
		return runtime.SessionContext{}, err
	}
	return runtime.Session(workspace, modelSpec, resolvedInteractive, resolvedSystemPrompt, systemFile, runtime.YoloEnabled(false), resolvedBestOf), nil
}

func TranscriptData(tx agent.Transcript) map[string]any {
	return map[string]any{
		"messages":           tx.Messages,
		"max_context_tokens": tx.MaxContextTokens,
		"max_message_tokens": tx.MaxMessageTokens,
	}
}

func LoadTranscript(data any) (agent.Transcript, error) {
	payload, ok := data.(map[string]any)
	if !ok {
		return agent.Transcript{}, fmt.Errorf("invalid transcript payload")
	}
	messagesAny, ok := payload["messages"].([]any)
	if !ok {
		return agent.Transcript{}, fmt.Errorf("invalid transcript messages")
	}
	messages := make([]providers.ChatMessage, 0, len(messagesAny))
	for _, item := range messagesAny {
		encoded, err := json.Marshal(item)
		if err != nil {
			return agent.Transcript{}, err
		}
		var msg providers.ChatMessage
		if err := json.Unmarshal(encoded, &msg); err != nil {
			return agent.Transcript{}, err
		}
		messages = append(messages, msg)
	}
	maxContext := runtime.MaxContextTokens()
	if value, ok := payload["max_context_tokens"].(float64); ok {
		maxContext = int(value)
	}
	maxMessage := runtime.DefaultBudgets().MessageTokens
	if value, ok := payload["max_message_tokens"].(float64); ok {
		maxMessage = int(value)
	}
	return agent.TranscriptState(messages, maxContext, maxMessage), nil
}

func SessionsPath() string {
	if SessionsDir != "" {
		return SessionsDir
	}
	return filepath.Join(filepath.Dir(runtime.ConfigPath()), "sessions")
}

func SessionFile(name string) string {
	safe := strings.Map(func(r rune) rune {
		if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') || r == '_' || r == '-' {
			return r
		}
		return '_'
	}, name)
	return filepath.Join(SessionsPath(), safe+".json")
}

func PackageJSONHasRenovateConfig(path string) bool {
	data, err := os.ReadFile(path)
	if err != nil {
		return false
	}
	var payload map[string]any
	if err := json.Unmarshal(data, &payload); err != nil {
		return false
	}
	_, ok := payload["renovate"]
	return ok
}

func ExistingRenovateConfig(workspace string) string {
	for _, relative := range RenovateConfigCandidates {
		candidate := filepath.Join(workspace, relative)
		if _, err := os.Stat(candidate); err == nil {
			return candidate
		}
	}
	packageJSON := filepath.Join(workspace, "package.json")
	if PackageJSONHasRenovateConfig(packageJSON) {
		return packageJSON
	}
	return ""
}

func EnsureRenovateConfig(workspace string) (string, bool, error) {
	if existing := ExistingRenovateConfig(workspace); existing != "" {
		return existing, false, nil
	}
	path := filepath.Join(workspace, "renovate.json")
	if err := os.WriteFile(path, []byte(DefaultRenovateConfig), 0o644); err != nil {
		return "", false, err
	}
	return path, true, nil
}

func ChatCommand(cmd string, tx *agent.Transcript, systemPrompt, modelSpec string) any {
	parts := strings.Fields(strings.TrimSpace(cmd))
	if len(parts) == 0 {
		return false
	}
	name := strings.ToLower(parts[0])
	arg := ""
	if len(parts) > 1 {
		arg = strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(cmd), parts[0]))
	}
	switch name {
	case "/help", "/?":
		lines := []string{"## Commands", ""}
		for _, item := range ChatCommandHelp[:len(ChatCommandHelp)-2] {
			lines = append(lines, fmt.Sprintf("- `%s` -- %s", item[0], item[1]))
		}
		lines = append(lines, "- `/quit` or `/exit` -- end session")
		_, _ = fmt.Fprintln(os.Stdout, strings.Join(lines, "\n"))
		return true
	case "/tokens":
		_, model := providers.SplitModelSpec(modelSpec)
		_ = model
		_, _ = fmt.Fprintf(os.Stdout, "## Context\n\n- messages: %d\n- session tokens: %s\n", len(tx.Messages), runtime.FormatTokens(agent.SessionTokens(*tx)))
		return true
	case "/model":
		return []string{"model", arg}
	case "/debug":
		return []string{"debug"}
	case "/yolo":
		return []string{"yolo"}
	case "/ask":
		return []string{"ask", arg}
	case "/audit":
		return []string{"audit", arg}
	case "/save":
		return []string{"save", arg}
	case "/load":
		return []string{"load", arg}
	case "/undo":
		return agent.UndoLastTurn(tx)
	case "/clear":
		agent.ClearTranscript(tx, systemPrompt)
		return true
	case "/quit", "/exit":
		return nil
	default:
		return false
	}
}

func HandleSave(name string, tx agent.Transcript, currentModel string) (string, error) {
	if err := providers.EnsurePrivateDir(SessionsPath()); err != nil {
		return "", err
	}
	if strings.TrimSpace(name) == "" {
		name = time.Now().Format("20060102-150405")
	}
	path := SessionFile(name)
	payload := map[string]any{
		"model":      currentModel,
		"saved_at":   time.Now().Format("2006-01-02T15:04:05"),
		"transcript": TranscriptData(tx),
	}
	if !providers.SaveJSON(path, payload) {
		return "", fmt.Errorf("could not save session")
	}
	return path, nil
}

func HandleLoad(name string, tx agent.Transcript, currentModel, systemPrompt string) (agent.Transcript, string, error) {
	if err := providers.EnsurePrivateDir(SessionsPath()); err != nil {
		return tx, currentModel, err
	}
	entries, err := os.ReadDir(SessionsPath())
	if err != nil {
		return tx, currentModel, err
	}
	sessions := []string{}
	for _, entry := range entries {
		if strings.HasSuffix(entry.Name(), ".json") {
			sessions = append(sessions, filepath.Join(SessionsPath(), entry.Name()))
		}
	}
	sort.Strings(sessions)
	if len(sessions) == 0 {
		return tx, currentModel, fmt.Errorf("no saved sessions found")
	}
	target := ""
	candidate := SessionFile(name)
	if _, err := os.Stat(candidate); err == nil {
		target = candidate
	} else {
		for _, session := range sessions {
			if strings.Contains(strings.ToLower(filepath.Base(session)), strings.ToLower(name)) {
				target = session
				break
			}
		}
	}
	if target == "" {
		return tx, currentModel, fmt.Errorf("no session found matching %s", name)
	}
	data, ok := providers.LoadJSON(target, nil).(map[string]any)
	if !ok {
		return tx, currentModel, fmt.Errorf("empty or invalid session file")
	}
	loaded, err := LoadTranscript(data["transcript"])
	if err != nil {
		return tx, currentModel, err
	}
	agent.SetSystemPrompt(&loaded, systemPrompt)
	loadedModel, _ := data["model"].(string)
	if loadedModel == "" {
		loadedModel = currentModel
	}
	return loaded, loadedModel, nil
}

func readTaskText(task []string) string {
	text := strings.TrimSpace(strings.Join(task, " "))
	if text == "" && !hasTTYStdinFunc() {
		text = strings.TrimSpace(readStdinFunc())
	}
	return text
}

func defaultRunAgent(prompt, model, root, systemPrompt string, unattendedLimitSeconds int, interactive, yolo bool, transcript *agent.Transcript, bestOf int) (int, string, error) {
	shim, err := providers.RequireAPIEnv(model, "", root)
	if err != nil {
		return 1, "", err
	}
	client, err := providers.GetClient(shim, root)
	if err != nil {
		return 1, "", err
	}
	return agent.RunAgent(client, prompt, model, root, systemPrompt, unattendedLimitSeconds, interactive, yolo, transcript, bestOf)
}

func Run(task ...string) int {
	taskText := readTaskText(task)
	if taskText == "" {
		return chatCommand()
	}
	session, err := resolveSessionFunc(boolPtr(false), "", true, nil)
	if err != nil {
		return 1
	}
	unattendedLimitSeconds, err := unattendedLimitFunc()
	if err != nil {
		return 1
	}
	code, _, err := runAgentFunc(taskText, session.Model, session.Workspace, session.SystemPrompt, unattendedLimitSeconds, session.Interactive, session.Yolo, nil, session.BestOf)
	if err != nil {
		return 1
	}
	return code
}

func Chat() int {
	return 0
}

func Ralph(task ...string) int {
	taskText := readTaskText(task)
	if taskText == "" {
		return 1
	}
	session, err := resolveSessionFunc(boolPtr(false), "", true, nil)
	if err != nil {
		return 1
	}
	session.Yolo = true
	limitSeconds, err := ralphLimitFunc()
	if err != nil {
		return 1
	}
	unattendedLimitSeconds, err := unattendedLimitFunc()
	if err != nil {
		return 1
	}
	deadline := nowFunc().Add(time.Duration(limitSeconds) * time.Second)
	delay := time.Minute
	exitCode := 0
	runNumber := 0
	for {
		now := nowFunc()
		if runNumber > 0 && !now.Before(deadline) {
			break
		}
		runNumber++
		code, _, err := runAgentFunc(taskText, session.Model, session.Workspace, session.SystemPrompt, unattendedLimitSeconds, session.Interactive, true, nil, session.BestOf)
		if err != nil {
			return 1
		}
		if code != 0 {
			exitCode = code
		}
		sleepFor := deadline.Sub(nowFunc())
		if sleepFor <= 0 {
			break
		}
		if sleepFor > delay {
			sleepFor = delay
		}
		sleepFunc(sleepFor)
	}
	return exitCode
}

func Audit(focus string) int {
	session, err := resolveSessionFunc(boolPtr(false), runtime.AuditSystemPrompt(), false, nil)
	if err != nil {
		return 1
	}
	_, _, err = EnsureRenovateConfig(session.Workspace)
	if err != nil {
		return 1
	}
	auditPrompt, err := runtime.SessionText(nil, "audit", "repo_user_prompt")
	if err != nil {
		return 1
	}
	if strings.TrimSpace(focus) != "" {
		suffix, err := runtime.SessionText(map[string]string{"focus": focus}, "audit", "focus_suffix")
		if err != nil {
			return 1
		}
		auditPrompt += suffix
	}
	unattendedLimitSeconds, err := unattendedLimitFunc()
	if err != nil {
		return 1
	}
	code, _, err := runAgentFunc(auditPrompt, session.Model, session.Workspace, session.SystemPrompt, unattendedLimitSeconds, false, false, nil, session.BestOf)
	if err != nil {
		return 1
	}
	return code
}

func Model(selection string) int {
	current, err := runtime.CurrentModel("")
	if err != nil && selection == "" {
		return 1
	}
	if selection == "" {
		_, _ = fmt.Fprintf(os.Stdout, "## Current Model\n\n- model: `%s`\n", current)
		return 0
	}
	if _, err := runtime.SaveModelConfig(selection); err != nil {
		return 1
	}
	return 0
}

func boolPtr(value bool) *bool { return &value }
