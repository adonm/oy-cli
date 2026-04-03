package agent

import (
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
	"github.com/wagov-dtt/oy-cli/internal/oy/tools"
)

type State struct {
	Root                    string
	ToolRegistry            map[string]tools.RegisteredTool
	UnattendedLimitSeconds  int
	UnattendedDeadline      time.Time
	Interactive             bool
	ApproveAllMutatingTools bool
	Yolo                    bool
	Todos                   []map[string]string
}

type Transcript struct {
	Messages         []providers.ChatMessage `json:"messages"`
	MaxContextTokens int                     `json:"max_context_tokens"`
	MaxMessageTokens int                     `json:"max_message_tokens"`
}

type CompletionRunner interface {
	ChatCompletion(model string, messages []providers.ChatMessage, specs []map[string]any, toolChoice string) (providers.ChatMessage, error)
}

var PrintFunc = func(value string) { fmt.Println(value) }
var NoteFunc = func(string) {}

func AgentState(root string, toolRegistry map[string]tools.RegisteredTool, unattendedLimitSeconds int, unattendedDeadline time.Time, interactive, approveAllMutatingTools, yolo bool, todos []map[string]string) State {
	copiedTodos := make([]map[string]string, 0, len(todos))
	for _, item := range todos {
		copied := map[string]string{}
		for key, value := range item {
			copied[key] = value
		}
		copiedTodos = append(copiedTodos, copied)
	}
	return State{
		Root:                    root,
		ToolRegistry:            toolRegistry,
		UnattendedLimitSeconds:  unattendedLimitSeconds,
		UnattendedDeadline:      unattendedDeadline,
		Interactive:             interactive,
		ApproveAllMutatingTools: approveAllMutatingTools,
		Yolo:                    yolo,
		Todos:                   copiedTodos,
	}
}

func NewAgentState(root string, toolRegistry map[string]tools.RegisteredTool, unattendedLimitSeconds int, interactive, yolo bool) State {
	return AgentState(
		root,
		toolRegistry,
		unattendedLimitSeconds,
		time.Now().Add(time.Duration(unattendedLimitSeconds)*time.Second),
		interactive,
		yolo,
		yolo,
		nil,
	)
}

func RemainingUnattendedSeconds(state State) float64 {
	return time.Until(state.UnattendedDeadline).Seconds()
}

func NoteProgress(state State) error {
	if RemainingUnattendedSeconds(state) <= 0 {
		return fmt.Errorf(
			"reached unattended timeout (%s) without a final response",
			formatDuration(state.UnattendedLimitSeconds),
		)
	}
	return nil
}

func TranscriptState(messages []providers.ChatMessage, maxContextTokens, maxMessageTokens int) Transcript {
	copied := append([]providers.ChatMessage(nil), messages...)
	return Transcript{Messages: copied, MaxContextTokens: maxContextTokens, MaxMessageTokens: maxMessageTokens}
}

func TranscriptWithSystemPrompt(systemPrompt string) Transcript {
	tx := TranscriptState(nil, runtime.MaxContextTokens(), runtime.DefaultBudgets().MessageTokens)
	SetSystemPrompt(&tx, systemPrompt)
	return tx
}

func SetSystemPrompt(tx *Transcript, systemPrompt string) {
	if len(tx.Messages) > 0 && tx.Messages[0].Role == "system" {
		tx.Messages[0] = providers.SystemMessage(systemPrompt)
		return
	}
	tx.Messages = append([]providers.ChatMessage{providers.SystemMessage(systemPrompt)}, tx.Messages...)
}

func ClearTranscript(tx *Transcript, systemPrompt string) {
	tx.Messages = nil
	SetSystemPrompt(tx, systemPrompt)
}

func Checkpoint(tx Transcript) int {
	return len(tx.Messages)
}

func Rollback(tx *Transcript, point int) {
	if point < 0 {
		point = 0
	}
	if point > len(tx.Messages) {
		point = len(tx.Messages)
	}
	tx.Messages = tx.Messages[:point]
}

func UndoLastTurn(tx *Transcript) bool {
	for index := len(tx.Messages) - 1; index > 0; index-- {
		if tx.Messages[index].Role == "user" {
			tx.Messages = tx.Messages[:index]
			return true
		}
	}
	return false
}

func AddUser(tx *Transcript, prompt string) {
	tx.Messages = append(tx.Messages, providers.UserMessage(prompt))
}

func AddAssistant(tx *Transcript, message providers.ChatMessage) {
	tx.Messages = append(tx.Messages, message)
}

func AddToolResults(tx *Transcript, results []map[string]any) {
	for _, result := range results {
		callID, _ := result["call_id"].(string)
		name, _ := result["name"].(string)
		toolResult, _ := result["result"].(providers.ToolResult)
		tx.Messages = append(tx.Messages, providers.ToolMessage(callID, name, toolResult))
	}
}

func PreparedMessages(tx Transcript, todos []map[string]string) []providers.ChatMessage {
	messages := append([]providers.ChatMessage(nil), tx.Messages...)
	systemMessages := []providers.ChatMessage{}
	for _, message := range messages {
		if message.Role == "system" {
			systemMessages = append(systemMessages, message)
		}
	}
	if len(todos) > 0 {
		todoText, _ := runtime.SessionText(map[string]string{"todos": formatTodos(todos)}, "transcript", "todo_system")
		systemMessages = append(systemMessages, providers.SystemMessage(strings.TrimSpace(todoText)))
	}
	other := []providers.ChatMessage{}
	for _, message := range messages {
		if message.Role != "system" {
			other = append(other, message)
		}
	}
	budget := tx.MaxContextTokens - len(systemMessages)
	if budget <= 0 {
		return systemMessages
	}
	kept := []providers.ChatMessage{}
	if len(other) > budget {
		omitted := len(other) - budget
		omittedText, _ := runtime.SessionText(map[string]string{"omitted_messages": fmt.Sprintf("%d", omitted)}, "transcript", "omitted_messages")
		kept = append(kept, providers.UserMessage(omittedText))
		other = other[len(other)-budget:]
	}
	kept = append(kept, other...)
	return append(systemMessages, kept...)
}

func SessionTokens(tx Transcript) int {
	return len(tx.Messages)
}

func PreparedTokens(tx Transcript, todos []map[string]string) int {
	return len(PreparedMessages(tx, todos))
}

func ChooseSelfConsistentMessage(messages []providers.ChatMessage) (providers.ChatMessage, int, int, error) {
	if len(messages) == 0 {
		return providers.ChatMessage{}, 0, 0, fmt.Errorf("messages must not be empty")
	}
	if len(messages) == 1 {
		return messages[0], 0, 1, nil
	}
	bestIndex := 0
	bestVotes := -1
	bestSupport := -1
	for index, candidate := range messages {
		votes := 0
		support := 0
		for _, other := range messages {
			score := messageMatchScore(candidate, other)
			support += score
			if score >= 85 {
				votes++
			}
		}
		if votes > bestVotes || (votes == bestVotes && (support > bestSupport || (support == bestSupport && index < bestIndex))) {
			bestIndex = index
			bestVotes = votes
			bestSupport = support
		}
	}
	return messages[bestIndex], bestIndex, bestVotes, nil
}

func RunTurn(client CompletionRunner, transcript *Transcript, state *State, modelSpec string, toolDefinitions []map[string]any, bestOf int) (int, string, error) {
	_, model := providers.SplitModelSpec(modelSpec)
	if bestOf <= 0 {
		return 1, "", fmt.Errorf("best_of must be a positive integer")
	}
	step := 0
	for {
		if err := NoteProgress(*state); err != nil {
			return 1, "", err
		}
		prepared := PreparedMessages(*transcript, state.Todos)
		runtime.DebugLog("request", map[string]any{
			"model":      modelSpec,
			"step":       step,
			"messages":   debugMessages(prepared),
			"tool_count": len(toolDefinitions),
		})
		responses := make([]providers.ChatMessage, 0, bestOf)
		for i := 0; i < bestOf; i++ {
			message, err := client.ChatCompletion(model, prepared, toolDefinitions, "auto")
			if err != nil {
				return 1, "", err
			}
			responses = append(responses, message)
		}
		message, chosenIndex, voteCount, err := ChooseSelfConsistentMessage(responses)
		if err != nil {
			return 1, "", err
		}
		runtime.DebugLog("response", map[string]any{
			"model":     modelSpec,
			"step":      step,
			"assistant": debugMessage(message),
		})
		if bestOf > 1 && voteCount < bestOf {
			NoteFunc(fmt.Sprintf("self-consistency: sample %d won %d/%d", chosenIndex+1, voteCount, bestOf))
		}
		if len(message.ToolCalls) > 0 {
			NoteFunc(fmt.Sprintf("turn %d: %s", step+1, tools.CountText(len(message.ToolCalls), "tool call", "")))
			AddAssistant(transcript, message)
			results := make([]map[string]any, 0, len(message.ToolCalls))
			for _, call := range message.ToolCalls {
				NoteFunc("tool " + formatToolCallPreview(call))
				toolState := &tools.State{
					Root:                    state.Root,
					Interactive:             state.Interactive,
					Yolo:                    state.Yolo,
					ApproveAllMutatingTools: state.ApproveAllMutatingTools,
					Todos:                   cloneTodos(state.Todos),
				}
				result := tools.InvokeTool(state.ToolRegistry, toolState, call.Name, call.Arguments)
				NoteFunc(fmt.Sprintf("tool %s: %s", call.Name, formatToolResultPreview(result)))
				state.ApproveAllMutatingTools = toolState.ApproveAllMutatingTools
				state.Todos = cloneTodos(toolState.Todos)
				results = append(results, map[string]any{"call_id": call.ID, "name": call.Name, "result": result})
			}
			runtime.DebugLog("tool_results", map[string]any{
				"model":   modelSpec,
				"step":    step,
				"results": debugToolResults(results),
			})
			AddToolResults(transcript, results)
			step++
			continue
		}
		PrintFunc(message.Content)
		return 0, message.Content, nil
	}
}

func RunAgent(client CompletionRunner, prompt, model, root, systemPrompt string, unattendedLimitSeconds int, interactive bool, yolo bool, transcript *Transcript, bestOf int) (int, string, error) {
	toolRegistry := tools.ActiveToolRegistry(interactive)
	if unattendedLimitSeconds <= 0 {
		return 1, "", fmt.Errorf("unattended_limit_seconds must be a positive integer")
	}
	if bestOf <= 0 {
		return 1, "", fmt.Errorf("best_of must be a positive integer")
	}
	state := NewAgentState(root, toolRegistry, unattendedLimitSeconds, interactive, yolo)
	if transcript == nil {
		tx := TranscriptWithSystemPrompt(systemPrompt)
		transcript = &tx
	} else {
		SetSystemPrompt(transcript, systemPrompt)
	}
	AddUser(transcript, prompt)
	return RunTurn(client, transcript, &state, model, tools.ToolSpecs(toolRegistry), bestOf)
}

func cloneTodos(todos []map[string]string) []map[string]string {
	out := make([]map[string]string, 0, len(todos))
	for _, item := range todos {
		copied := map[string]string{}
		for key, value := range item {
			copied[key] = value
		}
		out = append(out, copied)
	}
	return out
}

func debugMessage(message providers.ChatMessage) map[string]any {
	payload := map[string]any{"role": message.Role}
	if message.Content != "" {
		payload["content"] = message.Content
	}
	if len(message.ToolCalls) > 0 {
		calls := make([]map[string]any, 0, len(message.ToolCalls))
		for _, call := range message.ToolCalls {
			entry := map[string]any{"id": call.ID, "name": call.Name}
			if len(call.Arguments) > 0 {
				entry["arguments"] = call.Arguments
			}
			calls = append(calls, entry)
		}
		payload["tool_calls"] = calls
	}
	if len(message.ThoughtSignatures) > 0 {
		payload["thought_signatures"] = message.ThoughtSignatures
	}
	if message.ToolCallID != "" {
		payload["tool_call_id"] = message.ToolCallID
	}
	if message.Name != "" {
		payload["name"] = message.Name
	}
	return payload
}

func debugMessages(messages []providers.ChatMessage) []map[string]any {
	out := make([]map[string]any, 0, len(messages))
	for _, message := range messages {
		out = append(out, debugMessage(message))
	}
	return out
}

func debugToolResults(results []map[string]any) []map[string]any {
	out := make([]map[string]any, 0, len(results))
	for _, result := range results {
		entry := map[string]any{"call_id": result["call_id"], "name": result["name"]}
		if toolResult, ok := result["result"].(providers.ToolResult); ok {
			entry["ok"] = toolResult.OK
		}
		out = append(out, entry)
	}
	return out
}

func formatToolCallPreview(call providers.ToolCall) string {
	if len(call.Arguments) == 0 {
		return call.Name
	}
	keys := make([]string, 0, len(call.Arguments))
	for key := range call.Arguments {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	details := make([]string, 0, len(keys))
	for _, key := range keys {
		details = append(details, fmt.Sprintf("%s=%s", key, runtime.Preview(call.Arguments[key], 80)))
	}
	return fmt.Sprintf("%s(%s)", call.Name, strings.Join(details, ", "))
}

func formatToolResultPreview(result providers.ToolResult) string {
	status := "error"
	if result.OK {
		status = "ok"
	}
	preview := strings.TrimSpace(runtime.Preview(result.Content, 120))
	if preview == "" {
		return status
	}
	return fmt.Sprintf("%s %s", status, preview)
}

func normalizedVoteText(text string) string {
	clean := strings.Map(func(r rune) rune {
		if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') || (r >= '0' && r <= '9') {
			return r
		}
		return ' '
	}, text)
	return strings.ToLower(strings.Join(strings.Fields(clean), " "))
}

func textMatchScore(left, right string) int {
	if normalizedVoteText(left) == normalizedVoteText(right) {
		return 100
	}
	leftWords := strings.Fields(normalizedVoteText(left))
	rightWords := strings.Fields(normalizedVoteText(right))
	if len(leftWords) == 0 || len(rightWords) == 0 {
		return 0
	}
	matches := 0
	seen := map[string]struct{}{}
	for _, word := range leftWords {
		seen[word] = struct{}{}
	}
	for _, word := range rightWords {
		if _, ok := seen[word]; ok {
			matches++
		}
	}
	maxWords := len(leftWords)
	if len(rightWords) > maxWords {
		maxWords = len(rightWords)
	}
	return (matches * 100) / maxWords
}

func toolPlanMatchScore(left, right []providers.ToolCall) int {
	if len(left) == 0 || len(right) == 0 {
		if len(left) == len(right) {
			return 100
		}
		return 0
	}
	if len(left) != len(right) {
		return 0
	}
	score := 0
	for i := range left {
		if left[i].Name != right[i].Name {
			return 0
		}
		if providers.SerializeJSON(left[i].Arguments) == providers.SerializeJSON(right[i].Arguments) {
			score += 100
			continue
		}
		score += textMatchScore(providers.SerializeJSON(left[i].Arguments), providers.SerializeJSON(right[i].Arguments))
	}
	return score / len(left)
}

func messageMatchScore(left, right providers.ChatMessage) int {
	leftCalls := left.ToolCalls
	rightCalls := right.ToolCalls
	if (len(leftCalls) == 0) != (len(rightCalls) == 0) {
		return 0
	}
	if len(leftCalls) > 0 {
		plan := toolPlanMatchScore(leftCalls, rightCalls)
		if left.Content == "" && right.Content == "" {
			return plan
		}
		return (85*plan + 15*textMatchScore(left.Content, right.Content)) / 100
	}
	return textMatchScore(left.Content, right.Content)
}

func formatDuration(seconds int) string {
	if seconds%3600 == 0 {
		return fmt.Sprintf("%dh", seconds/3600)
	}
	if seconds%60 == 0 {
		return fmt.Sprintf("%dm", seconds/60)
	}
	return fmt.Sprintf("%ds", seconds)
}

func formatTodos(todos []map[string]string) string {
	lines := make([]string, 0, len(todos))
	for _, item := range todos {
		status := item["status"]
		icon := map[string]string{"pending": "[ ]", "in_progress": "[~]", "done": "[x]"}[status]
		if icon == "" {
			icon = "[ ]"
		}
		line := fmt.Sprintf("%s %s: %s", icon, strings.TrimSpace(item["id"]), strings.TrimSpace(item["task"]))
		lines = append(lines, line)
	}
	return strings.Join(lines, "\n")
}
