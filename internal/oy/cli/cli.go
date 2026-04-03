package cli

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/agent"
	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
	"github.com/wagov-dtt/oy-cli/internal/oy/tools"
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
	canPromptFunc      = runtime.CanPrompt
	readStdinFunc      = func() string {
		data, _ := io.ReadAll(os.Stdin)
		return strings.TrimSpace(string(data))
	}
	unattendedLimitFunc  = runtime.UnattendedLimitSeconds
	ralphLimitFunc       = runtime.RalphLimitSeconds
	requireAPIEnvFunc    = providers.RequireAPIEnv
	getClientFunc        = providers.GetClient
	listAllModelIDsFunc  = runtime.ListAllModelIDs
	chatInputReaderFunc  = func() io.Reader { return os.Stdin }
	modelInputReaderFunc = func() io.Reader { return os.Stdin }
	stderrIsTTYFunc      = func() bool {
		file, ok := stderrWriter.(*os.File)
		if !ok {
			return false
		}
		info, err := file.Stat()
		if err != nil {
			return false
		}
		return (info.Mode() & os.ModeCharDevice) != 0
	}
	gitDiffShortstatFunc           = gitDiffShortstat
	stdoutWriter         io.Writer = os.Stdout
	stderrWriter         io.Writer = os.Stderr
	nowFunc                        = time.Now
	sleepFunc                      = time.Sleep
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

var TopLevelCommandHelp = [][2]string{
	{"run", "Run a one-shot task."},
	{"chat", "Start an interactive multi-turn chat session."},
	{"ralph", "Run a task in yolo mode every minute until the configured deadline."},
	{"model", "Show or change the default model."},
	{"audit", "Run a one-shot security and complexity audit."},
}

type sessionField struct {
	Key   string
	Value string
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
		fmt.Fprintf(stdoutWriter, "oy %s\n", version.Version)
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
		return runChatCommand(args[1:])
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
	fmt.Fprintln(stdoutWriter, "usage: oy [-h] [--version] {run,chat,ralph,model,audit} ...")
	fmt.Fprintln(stdoutWriter)
	fmt.Fprintln(stdoutWriter, "AI coding assistant for your shell.")
	fmt.Fprintln(stdoutWriter)
	fmt.Fprintln(stdoutWriter, "positional arguments:")
	fmt.Fprintln(stdoutWriter, "  {run,chat,ralph,model,audit}")
	for _, item := range TopLevelCommandHelp {
		fmt.Fprintf(stdoutWriter, "    %-20s %s\n", item[0], item[1])
	}
	fmt.Fprintln(stdoutWriter)
	fmt.Fprintln(stdoutWriter, "options:")
	fmt.Fprintln(stdoutWriter, "  -h, --help            show this help message and exit")
	fmt.Fprintln(stdoutWriter, "  --version             show program's version number and exit")
	fmt.Fprintln(stdoutWriter)
	fmt.Fprintln(stdoutWriter, "Examples:")
	fmt.Fprintln(stdoutWriter, "  oy \"fix the failing tests\"")
	fmt.Fprintln(stdoutWriter, "  oy run \"fix the flaky test\"")
	fmt.Fprintln(stdoutWriter, "  oy chat")
	fmt.Fprintln(stdoutWriter, "  oy chat --yolo")
	fmt.Fprintln(stdoutWriter, "  oy ralph \"fix the flaky test\"")
	fmt.Fprintln(stdoutWriter, "  oy audit auth")
	fmt.Fprintln(stdoutWriter, "  oy model gpt-5")
}

func runChatCommand(args []string) int {
	yolo := false
	for _, arg := range args {
		switch strings.TrimSpace(arg) {
		case "", "chat":
			continue
		case "--yolo":
			yolo = true
		case "-h", "--help":
			fmt.Fprintln(stdoutWriter, "usage: oy chat [-h] [--yolo]")
			fmt.Fprintln(stdoutWriter)
			fmt.Fprintln(stdoutWriter, "Start an interactive multi-turn chat session.")
			fmt.Fprintln(stdoutWriter)
			fmt.Fprintln(stdoutWriter, "options:")
			fmt.Fprintln(stdoutWriter, "  -h, --help  show this help message and exit")
			fmt.Fprintln(stdoutWriter, "  --yolo      Allow all tools without per-action approval prompts.")
			return 0
		default:
			fmt.Fprintf(stderrWriter, "[error] unknown chat option: %s\n", arg)
			return 1
		}
	}
	if !yolo {
		return chatCommand()
	}
	previous, hadPrevious := os.LookupEnv("OY_YOLO")
	_ = os.Setenv("OY_YOLO", "1")
	defer func() {
		if hadPrevious {
			_ = os.Setenv("OY_YOLO", previous)
		} else {
			_ = os.Unsetenv("OY_YOLO")
		}
	}()
	return chatCommand()
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

func printSessionIntro(heading string, session runtime.SessionContext, extras ...sessionField) {
	lines := []string{
		fmt.Sprintf("## %s", heading),
		"",
		fmt.Sprintf("- workspace: `%s`", session.Workspace),
		fmt.Sprintf("- model: `%s`", session.Model),
		fmt.Sprintf("- mode: `%s`", map[bool]string{true: "interactive", false: "non-interactive"}[session.Interactive]),
	}
	if session.SystemFile != "" {
		systemFile := session.SystemFile
		if resolved, err := filepath.Abs(systemFile); err == nil {
			systemFile = resolved
		}
		lines = append(lines, fmt.Sprintf("- system file: `%s`", systemFile))
	}
	for _, extra := range extras {
		if strings.TrimSpace(extra.Value) != "" {
			lines = append(lines, fmt.Sprintf("- %s: `%s`", extra.Key, extra.Value))
		}
	}
	if debugPath := runtime.DebugLogPath(); debugPath != "" {
		lines = append(lines, fmt.Sprintf("- debug log: `%s`", debugPath))
	}
	fmt.Fprintln(stderrWriter, strings.Join(lines, "\n"))
	applySessionTitle(session.Workspace, session.Model)
}

func applySessionTitle(workspace, modelSpec string) {
	_, model := providers.SplitModelSpec(modelSpec)
	setTerminalTitle(fmt.Sprintf("oy · %s · %s", model, filepath.Base(workspace)))
}

func setTerminalTitle(title string) {
	if !stderrIsTTYFunc() {
		return
	}
	fmt.Fprintf(stderrWriter, "]0;%s", title)
}

func gitDiffShortstat(workspace string) string {
	result, err := providers.RunCmd(
		[]string{"git", "-C", workspace, "diff", "--shortstat", "--no-ext-diff", "HEAD", "--"},
		"",
		nil,
		5*time.Second,
		"",
	)
	if err != nil || result.ReturnCode != 0 {
		return ""
	}
	summary := strings.TrimSpace(result.Stdout)
	if summary == "" {
		return "git diff: clean"
	}
	return summary
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
		lines = append(lines,
			"- `/quit` or `/exit` -- end session",
			"",
			"Older conversation history may be packed into TOON before model requests.",
		)
		fmt.Fprintln(stdoutWriter, strings.Join(lines, "\n"))
		return true
	case "/tokens":
		prepared := agent.PreparedTokens(*tx, nil)
		fmt.Fprintf(
			stdoutWriter,
			"## Context\n\n- messages: %d\n- session tokens: %s\n- prepared tokens: %s\n- context budget: %s\n- remaining: ~%s\n",
			len(tx.Messages),
			runtime.FormatTokens(agent.SessionTokens(*tx)),
			runtime.FormatTokens(prepared),
			runtime.FormatTokens(tx.MaxContextTokens),
			runtime.FormatTokens(maxInt(tx.MaxContextTokens-prepared, 0)),
		)
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
	sessions, err := savedSessions()
	if err != nil {
		return tx, currentModel, err
	}
	if len(sessions) == 0 {
		return tx, currentModel, fmt.Errorf("no saved sessions found")
	}
	if strings.TrimSpace(name) == "" {
		return tx, currentModel, fmt.Errorf("load requires a session name or index")
	}
	target := resolveSavedSession(name, sessions)
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
	shim, err := requireAPIEnvFunc(model, "", root)
	if err != nil {
		return 1, "", err
	}
	client, err := getClientFunc(shim, root)
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
	code, _, err := runAgentFunc(taskText, session.Model, session.Workspace, session.SystemPrompt, unattendedLimitSeconds, session.Interactive, true, nil, session.BestOf)
	if err != nil {
		return 1
	}
	return code
}

func Chat() int {
	session, err := resolveSessionFunc(boolPtr(true), "", true, nil)
	if err != nil {
		return 1
	}
	printSessionIntro("Chat", session, sessionField{Key: "best-of", Value: strconv.Itoa(session.BestOf)})
	fmt.Fprintf(stderrWriter, "[note] chat mode; /help for commands%s\n", map[bool]string{true: "; yolo on", false: ""}[session.Yolo])

	transcript := agent.TranscriptWithSystemPrompt(session.SystemPrompt)
	currentModel := session.Model
	scanner := bufio.NewScanner(chatInputReaderFunc())
	scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
	for {
		if summary := gitDiffShortstatFunc(session.Workspace); summary != "" {
			fmt.Fprintln(stderrWriter, summary)
		}
		fmt.Fprint(stderrWriter, "oy > ")
		if !scanner.Scan() {
			if err := scanner.Err(); err != nil {
				fmt.Fprintf(stderrWriter, "[error] input error: %v\n", err)
			}
			fmt.Fprintln(stderrWriter, "[note] session ended")
			break
		}
		prompt := strings.TrimSpace(scanner.Text())
		if prompt == "" {
			continue
		}
		if strings.HasPrefix(prompt, "/") {
			name := strings.ToLower(strings.Fields(prompt)[0])
			result := ChatCommand(prompt, &transcript, session.SystemPrompt, currentModel)
			if result == nil {
				break
			}
			if action, ok := result.([]string); ok {
				switch action[0] {
				case "model":
					currentModel = handleModelSwitch(strings.TrimSpace(strings.Join(action[1:], " ")), currentModel, session.Workspace, stderrWriter)
				case "debug":
					handleDebugToggle()
				case "yolo":
					if session.Yolo {
						fmt.Fprintln(stderrWriter, "[note] yolo already enabled for this session")
					} else {
						session.Yolo = true
						fmt.Fprintln(stderrWriter, "[note] yolo enabled; all tools allowed for this session")
					}
				case "ask":
					handleAsk(strings.TrimSpace(strings.Join(action[1:], " ")), currentModel, session, transcript)
				case "audit":
					handleAudit(strings.TrimSpace(strings.Join(action[1:], " ")), currentModel, session)
				case "save":
					path, err := HandleSave(strings.TrimSpace(strings.Join(action[1:], " ")), transcript, currentModel)
					if err != nil {
						fmt.Fprintf(stderrWriter, "[error] Failed to save session: %v\n", err)
						continue
					}
					fmt.Fprintf(stderrWriter, "[note] saved session: %s\n", filepath.Base(path))
				case "load":
					arg := strings.TrimSpace(strings.Join(action[1:], " "))
					if arg == "" {
						printSavedSessions()
						continue
					}
					loaded, model, err := HandleLoad(arg, transcript, currentModel, session.SystemPrompt)
					if err != nil {
						fmt.Fprintf(stderrWriter, "[error] Failed to load session: %v\n", err)
						continue
					}
					transcript = loaded
					currentModel = model
					fmt.Fprintf(stderrWriter, "[note] loaded session (%d messages, model: %s)\n", len(transcript.Messages), currentModel)
				}
				continue
			}
			if handled, ok := result.(bool); ok {
				if !handled {
					fmt.Fprintf(stderrWriter, "[warn] Unknown command: %s\n", name)
					continue
				}
				switch name {
				case "/undo":
					fmt.Fprintln(stderrWriter, map[bool]string{true: "[note] undid last turn", false: "[warn] Nothing to undo."}[result.(bool)])
				case "/clear":
					fmt.Fprintln(stderrWriter, "[note] cleared conversation")
				}
				continue
			}
			continue
		}
		if strings.EqualFold(prompt, "exit") || strings.EqualFold(prompt, "quit") {
			break
		}
		unattendedLimitSeconds, err := unattendedLimitFunc()
		if err != nil {
			fmt.Fprintf(stderrWriter, "[error] Agent error: %v\n", err)
			return 1
		}
		checkpoint := agent.Checkpoint(transcript)
		code, _, err := runAgentFunc(prompt, currentModel, session.Workspace, session.SystemPrompt, unattendedLimitSeconds, session.Interactive, session.Yolo, &transcript, session.BestOf)
		if err != nil {
			agent.Rollback(&transcript, checkpoint)
			fmt.Fprintf(stderrWriter, "[error] Agent error: %v\n", err)
			continue
		}
		_ = code
		used := agent.PreparedTokens(transcript, nil)
		remaining := maxInt(transcript.MaxContextTokens-used, 0)
		fmt.Fprintf(stderrWriter, "[note] context: %s used, ~%s remaining\n", runtime.FormatTokens(used), runtime.FormatTokens(remaining))
	}
	setTerminalTitle("")
	return 0
}

func Ralph(task ...string) int {
	taskText := readTaskText(task)
	if taskText == "" {
		fmt.Fprintln(stderrWriter, "Usage: `oy ralph <prompt>` — or pipe prompt text on stdin.")
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
	delay := time.Minute
	printSessionIntro(
		"Ralph",
		session,
		sessionField{Key: "prompt", Value: runtime.Preview(taskText, 100)},
		sessionField{Key: "schedule", Value: fmt.Sprintf("until %s deadline, %s delay", runtime.FormatDuration(limitSeconds), runtime.FormatDuration(int(delay/time.Second)))},
	)
	deadline := nowFunc().Add(time.Duration(limitSeconds) * time.Second)
	exitCode := 0
	runNumber := 0
	for {
		now := nowFunc()
		if runNumber > 0 && !now.Before(deadline) {
			break
		}
		runNumber++
		remaining := maxInt(int(deadline.Sub(now).Seconds()), 0)
		fmt.Fprintf(stderrWriter, "[note] ralph run %d (~%s remaining)\n", runNumber, runtime.FormatDuration(remaining))
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
	path, created, err := EnsureRenovateConfig(session.Workspace)
	if err != nil {
		return 1
	}
	if created {
		fmt.Fprintf(stderrWriter, "[note] created default Renovate config: %s\n", filepath.Base(path))
	}
	auditPrompt, err := runtime.SessionText(nil, "audit", "repo_user_prompt")
	if err != nil {
		return 1
	}
	focus = strings.TrimSpace(focus)
	if focus != "" {
		suffix, err := runtime.SessionText(map[string]string{"focus": focus}, "audit", "focus_suffix")
		if err != nil {
			return 1
		}
		auditPrompt += suffix
	}
	extras := []sessionField{}
	if focus != "" {
		extras = append(extras, sessionField{Key: "focus", Value: runtime.Preview(focus, 100)})
	}
	printSessionIntro("Audit", session, extras...)
	unattendedLimitSeconds, err := unattendedLimitFunc()
	if err != nil {
		return 1
	}
	fmt.Fprintln(stderrWriter, "[note] audit mode")
	code, _, err := runAgentFunc(auditPrompt, session.Model, session.Workspace, session.SystemPrompt, unattendedLimitSeconds, false, false, nil, session.BestOf)
	if err != nil {
		return 1
	}
	return code
}

func Model(selection string) int {
	current, err := runtime.CurrentModel("")
	if err != nil && strings.TrimSpace(selection) == "" && !canPromptFunc() {
		return 1
	}
	if err != nil {
		current = ""
	}
	if strings.TrimSpace(selection) == "" && !canPromptFunc() {
		fmt.Fprintln(stdoutWriter, currentModelText(current))
		return 0
	}
	input := bufio.NewReader(modelInputReaderFunc())
	if current != "" && canPromptFunc() {
		fmt.Fprintln(stderrWriter, currentModelText(current))
		if strings.TrimSpace(selection) == "" {
			pickNew, err := promptYesNo(input, stderrWriter, "Pick a new model?", false)
			if err != nil {
				fmt.Fprintf(stderrWriter, "[error] Failed to read model selection: %v\n", err)
				return 1
			}
			if !pickNew {
				return 0
			}
		}
	}
	workspace, _ := WorkspaceRoot()
	selected, err := resolveModelChoice(selection, current, workspace, input, stderrWriter)
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] %v\n", err)
		return 1
	}
	if strings.TrimSpace(selected) == "" {
		return 1
	}
	config, err := runtime.SaveModelConfig(selected)
	if err != nil {
		return 1
	}
	fmt.Fprintf(stdoutWriter, "## Default Model Updated\n\n- selected: `%s`\n", selected)
	if config.Shim != "" {
		fmt.Fprintf(stdoutWriter, "- shim: `%s`\n", config.Shim)
	}
	setTerminalTitle("")
	return 0
}

func savedSessions() ([]string, error) {
	if err := providers.EnsurePrivateDir(SessionsPath()); err != nil {
		return nil, err
	}
	entries, err := os.ReadDir(SessionsPath())
	if err != nil {
		return nil, err
	}
	sessions := []string{}
	for _, entry := range entries {
		if strings.HasSuffix(entry.Name(), ".json") {
			sessions = append(sessions, filepath.Join(SessionsPath(), entry.Name()))
		}
	}
	sort.SliceStable(sessions, func(i, j int) bool {
		left, leftErr := os.Stat(sessions[i])
		right, rightErr := os.Stat(sessions[j])
		if leftErr != nil || rightErr != nil || left.ModTime().Equal(right.ModTime()) {
			return sessions[i] > sessions[j]
		}
		return left.ModTime().After(right.ModTime())
	})
	return sessions, nil
}

func resolveSavedSession(name string, sessions []string) string {
	if index, ok := parseIndex(name); ok && index >= 0 && index < len(sessions) {
		return sessions[index]
	}
	candidate := SessionFile(name)
	if _, err := os.Stat(candidate); err == nil {
		return candidate
	}
	matches := []string{}
	needle := strings.ToLower(strings.TrimSpace(name))
	for _, session := range sessions {
		stem := strings.TrimSuffix(filepath.Base(session), ".json")
		if strings.Contains(strings.ToLower(stem), needle) {
			matches = append(matches, session)
		}
	}
	if len(matches) == 1 {
		return matches[0]
	}
	return ""
}

func printSavedSessions() {
	sessions, err := savedSessions()
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Failed to list sessions: %v\n", err)
		return
	}
	if len(sessions) == 0 {
		fmt.Fprintln(stderrWriter, "[warn] No saved sessions found.")
		return
	}
	lines := []string{"## Saved Sessions", ""}
	for index, session := range sessions {
		if index >= 20 {
			break
		}
		model := "?"
		savedAt := "?"
		msgCount := 0
		if meta, ok := providers.LoadJSON(session, map[string]any{}).(map[string]any); ok {
			if value, ok := meta["model"].(string); ok && value != "" {
				model = value
			}
			if value, ok := meta["saved_at"].(string); ok && value != "" {
				savedAt = value
			}
			if transcript, ok := meta["transcript"].(map[string]any); ok {
				if messages, ok := transcript["messages"].([]any); ok {
					msgCount = len(messages)
				}
			}
		}
		lines = append(lines, fmt.Sprintf("%d. `%s` — %s, %d msgs, %s", index+1, strings.TrimSuffix(filepath.Base(session), ".json"), model, msgCount, savedAt))
	}
	lines = append(lines, "", "Usage: `/load <name>` or `/load <number>`")
	fmt.Fprintln(stderrWriter, strings.Join(lines, "\n"))
}

func handleModelSwitch(arg, currentModel, cwd string, output io.Writer) string {
	arg = strings.TrimSpace(arg)
	if arg == "" {
		fmt.Fprintln(output, currentModelText(currentModel))
		fmt.Fprintln(output, "[note] use /model <name> to switch, or /model list to browse")
		return currentModel
	}
	allModels, warnings, err := listAllModelIDsFunc(cwd)
	if err != nil {
		fmt.Fprintf(output, "[warn] Could not load model list: %v\n", err)
		return currentModel
	}
	for _, warning := range warnings {
		fmt.Fprintf(output, "[warn] %s\n", warning)
	}
	if strings.EqualFold(arg, "list") {
		printModelList(output, "## Available Models", allModels)
		return currentModel
	}
	for _, model := range allModels {
		if model == arg {
			fmt.Fprintf(output, "[note] switched model: %s\n", model)
			return model
		}
	}
	matches := filterModels(allModels, arg)
	if len(matches) == 1 {
		fmt.Fprintf(output, "[note] switched model: %s\n", matches[0])
		return matches[0]
	}
	if len(matches) > 1 {
		printModelList(output, "## Matching Models", matches)
		fmt.Fprintln(output, "Be more specific or use `/model list` to choose interactively.")
		return currentModel
	}
	fmt.Fprintf(output, "[warn] No models matching `%s`.\n", arg)
	return currentModel
}

func handleDebugToggle() {
	if runtime.DebugLogPath() != "" {
		os.Unsetenv("OY_DEBUG")
		if err := runtime.DisableDebugLog(); err != nil {
			fmt.Fprintf(stderrWriter, "[error] Failed to disable debug logging: %v\n", err)
			return
		}
		fmt.Fprintln(stderrWriter, "[note] debug logging disabled")
		return
	}
	os.Setenv("OY_DEBUG", "1")
	path, err := runtime.InitDebugLog()
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Failed to enable debug logging: %v\n", err)
		return
	}
	fmt.Fprintf(stderrWriter, "[note] debug logging enabled: %s\n", path)
}

func handleAsk(question, currentModel string, session runtime.SessionContext, tx agent.Transcript) {
	question = strings.TrimSpace(question)
	if question == "" {
		fmt.Fprintln(stderrWriter, AskUsage)
		return
	}
	unattendedLimitSeconds, err := unattendedLimitFunc()
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Research error: %v\n", err)
		return
	}
	askTranscript := agent.TranscriptWithSystemPrompt(runtime.AskSystemPrompt(session.SystemPrompt))
	start := maxInt(len(tx.Messages)-6, 0)
	for _, message := range tx.Messages[start:] {
		if message.Role != "system" {
			askTranscript.Messages = append(askTranscript.Messages, message)
		}
	}
	agent.AddUser(&askTranscript, question)
	registry := tools.ReadOnlyToolRegistry()
	state := agent.NewAgentState(session.Workspace, registry, unattendedLimitSeconds, session.Interactive, false)
	shim, err := requireAPIEnvFunc(currentModel, "", session.Workspace)
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Research error: %v\n", err)
		return
	}
	client, err := getClientFunc(shim, session.Workspace)
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Research error: %v\n", err)
		return
	}
	fmt.Fprintf(stderrWriter, "[note] %s\n", AskModeNote)
	if _, _, err := agent.RunTurn(client, &askTranscript, &state, currentModel, tools.ToolSpecs(registry), session.BestOf); err != nil {
		fmt.Fprintf(stderrWriter, "[error] Research error: %v\n", err)
	}
}

func handleAudit(focus, currentModel string, session runtime.SessionContext) {
	if _, _, err := EnsureRenovateConfig(session.Workspace); err != nil {
		fmt.Fprintf(stderrWriter, "[error] Audit error: %v\n", err)
		return
	}
	auditPrompt, err := runtime.SessionText(nil, "audit", "repo_user_prompt")
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Audit error: %v\n", err)
		return
	}
	if strings.TrimSpace(focus) != "" {
		suffix, err := runtime.SessionText(map[string]string{"focus": focus}, "audit", "focus_suffix")
		if err != nil {
			fmt.Fprintf(stderrWriter, "[error] Audit error: %v\n", err)
			return
		}
		auditPrompt += suffix
	}
	unattendedLimitSeconds, err := unattendedLimitFunc()
	if err != nil {
		fmt.Fprintf(stderrWriter, "[error] Audit error: %v\n", err)
		return
	}
	fmt.Fprintln(stderrWriter, "[note] audit mode")
	if _, _, err := runAgentFunc(auditPrompt, currentModel, session.Workspace, runtime.AuditSystemPrompt(), unattendedLimitSeconds, false, false, nil, session.BestOf); err != nil {
		fmt.Fprintf(stderrWriter, "[error] Audit error: %v\n", err)
	}
}

func currentModelText(modelSpec string) string {
	shim := providers.ResolveShim(modelSpec, runtime.LoadModelConfig().Shim)
	_, bare := providers.SplitModelSpec(modelSpec)
	return fmt.Sprintf("## Current Model\n\n- model: `%s`\n- shim: `%s`", bare, shim)
}

func resolveModelChoice(selection, currentModel, cwd string, input io.Reader, output io.Writer) (string, error) {
	allModels, warnings, err := listAllModelIDsFunc(cwd)
	if err != nil {
		return "", err
	}
	for _, warning := range warnings {
		fmt.Fprintf(output, "[warn] %s\n", warning)
	}
	if len(allModels) == 0 {
		return "", fmt.Errorf("no models available")
	}
	selection = strings.TrimSpace(selection)
	for _, model := range allModels {
		if model == selection {
			return model, nil
		}
	}
	if !canPromptFunc() {
		if selection == "" {
			return "", nil
		}
		matches := filterModels(allModels, selection)
		if len(matches) > 0 {
			printModelList(output, "## Matching Models", matches)
		}
		return "", fmt.Errorf("No exact model match for `%s`. Re-run in a TTY to filter and choose interactively.", selection)
	}
	fmt.Fprintln(output, "## Choose a Model\n\n- Enter an exact model ID to save it.\n- Enter text to filter the list.\n- Enter a number to pick from the currently listed models.")
	shown := append([]string(nil), allModels...)
	if selection == "" {
		printSelectableModelList(output, "## Available Models", shown, currentModel)
		selection, err = promptLine(input, output, "Model or filter", currentModel)
		if err != nil {
			return "", err
		}
	}
	for {
		selection = strings.TrimSpace(selection)
		if selection == "" {
			selection = currentModel
		}
		for _, model := range allModels {
			if model == selection {
				return model, nil
			}
		}
		if index, ok := parseIndex(selection); ok && index >= 0 && index < len(shown) {
			return shown[index], nil
		}
		shown = filterModels(allModels, selection)
		printSelectableModelList(output, "## Matching Models", shown, currentModel)
		selection, err = promptLine(input, output, "Model or filter", "")
		if err != nil {
			return "", err
		}
	}
}

func printModelList(output io.Writer, title string, models []string) {
	lines := []string{title, ""}
	if len(models) == 0 {
		lines = append(lines, "(no matches)")
	} else {
		for _, model := range models {
			lines = append(lines, fmt.Sprintf("- `%s`", model))
		}
	}
	fmt.Fprintln(output, strings.Join(lines, "\n"))
}

func printSelectableModelList(output io.Writer, title string, models []string, currentModel string) {
	lines := []string{title, ""}
	if len(models) == 0 {
		lines = append(lines, "(no matches)")
	} else {
		for index, model := range models {
			suffix := ""
			if model == currentModel {
				suffix = " — current"
			}
			lines = append(lines, fmt.Sprintf("%d. `%s`%s", index+1, model, suffix))
		}
	}
	fmt.Fprintln(output, strings.Join(lines, "\n"))
}

func promptLine(input io.Reader, output io.Writer, label, defaultValue string) (string, error) {
	if defaultValue != "" {
		fmt.Fprintf(output, "%s [%s]: ", label, defaultValue)
	} else {
		fmt.Fprintf(output, "%s: ", label)
	}
	reader, ok := input.(*bufio.Reader)
	if !ok {
		reader = bufio.NewReader(input)
	}
	text, err := reader.ReadString('\n')
	if err != nil && err != io.EOF {
		return "", err
	}
	text = strings.TrimSpace(text)
	if text == "" {
		return defaultValue, nil
	}
	return text, nil
}

func promptYesNo(input io.Reader, output io.Writer, label string, defaultYes bool) (bool, error) {
	defaultChoice := "n"
	if defaultYes {
		defaultChoice = "y"
	}
	choice, err := promptLine(input, output, label, defaultChoice)
	if err != nil {
		return false, err
	}
	switch strings.ToLower(strings.TrimSpace(choice)) {
	case "y", "yes":
		return true, nil
	case "", "n", "no":
		return false, nil
	default:
		return false, nil
	}
}

func filterModels(models []string, query string) []string {
	query = strings.ToLower(strings.TrimSpace(query))
	if query == "" {
		return append([]string(nil), models...)
	}
	matches := []string{}
	for _, model := range models {
		if strings.Contains(strings.ToLower(model), query) {
			matches = append(matches, model)
		}
	}
	return matches
}

func parseIndex(value string) (int, bool) {
	value = strings.TrimSpace(value)
	if value == "" {
		return 0, false
	}
	index, err := strconv.Atoi(value)
	if err != nil || index <= 0 {
		return 0, false
	}
	return index - 1, true
}

func maxInt(left, right int) int {
	if left > right {
		return left
	}
	return right
}

func boolPtr(value bool) *bool { return &value }
