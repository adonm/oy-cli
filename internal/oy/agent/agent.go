package agent

import (
	"fmt"
	"strings"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
)

type State struct {
	Root                    string
	ToolRegistry            map[string]any
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

func AgentState(root string, toolRegistry map[string]any, unattendedLimitSeconds int, unattendedDeadline time.Time, interactive, approveAllMutatingTools, yolo bool, todos []map[string]string) State {
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

func NewAgentState(root string, toolRegistry map[string]any, unattendedLimitSeconds int, interactive, yolo bool) State {
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
