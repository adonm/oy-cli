package agent

import (
	"strings"
	"testing"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/tools"
)

type stubCompletionClient struct {
	responses []providers.ChatMessage
	index     int
	requests  [][]providers.ChatMessage
}

func (s *stubCompletionClient) ChatCompletion(model string, messages []providers.ChatMessage, specs []map[string]any, toolChoice string) (providers.ChatMessage, error) {
	_ = model
	_ = specs
	_ = toolChoice
	s.requests = append(s.requests, append([]providers.ChatMessage(nil), messages...))
	message := s.responses[s.index]
	s.index++
	return message, nil
}

func TestTranscriptLifecycle(t *testing.T) {
	tx := TranscriptWithSystemPrompt("sys")
	AddUser(&tx, "hello")
	ClearTranscript(&tx, "next")
	if len(tx.Messages) != 1 || tx.Messages[0].Role != "system" || tx.Messages[0].Content != "next" {
		t.Fatalf("unexpected transcript after clear: %#v", tx.Messages)
	}
	if UndoLastTurn(&tx) {
		t.Fatal("expected undo to be false")
	}
}

func TestCheckpointRollbackAndUndo(t *testing.T) {
	tx := TranscriptWithSystemPrompt("sys")
	AddUser(&tx, "hello")
	point := Checkpoint(tx)
	AddUser(&tx, "more")
	Rollback(&tx, point)
	if len(tx.Messages) != point {
		t.Fatalf("unexpected rollback size: %d", len(tx.Messages))
	}
	if !UndoLastTurn(&tx) {
		t.Fatal("expected undo to remove last turn")
	}
	if len(tx.Messages) != 1 {
		t.Fatalf("unexpected transcript after undo: %#v", tx.Messages)
	}
}

func TestPreparedMessagesAddsTodoAndOmittedNote(t *testing.T) {
	tx := TranscriptState([]providers.ChatMessage{
		providers.SystemMessage("sys"),
		providers.UserMessage("one"),
		providers.UserMessage("two"),
		providers.UserMessage("three"),
	}, 3, 100)
	prepared := PreparedMessages(tx, []map[string]string{{"id": "t1", "task": "ship it", "status": "in_progress"}})
	if len(prepared) != 4 {
		t.Fatalf("unexpected prepared length: %#v", prepared)
	}
	if prepared[0].Role != "system" || prepared[1].Role != "system" {
		t.Fatalf("expected system messages first: %#v", prepared)
	}
	if prepared[2].Role != "user" || prepared[2].Content == "" {
		t.Fatalf("expected omitted note: %#v", prepared)
	}
	if prepared[3].Role != "user" || prepared[3].Content != "three" {
		t.Fatalf("expected latest user message kept: %#v", prepared)
	}
}

func TestNoteProgressTimesOut(t *testing.T) {
	state := AgentState("/tmp", nil, 60, time.Now().Add(-time.Second), false, false, false, nil)
	if err := NoteProgress(state); err == nil {
		t.Fatal("expected timeout error")
	}
}

func TestRunTurnExecutesToolCallsUntilFinalAnswer(t *testing.T) {
	registry := map[string]tools.RegisteredTool{
		"echo": {
			Spec:     tools.Spec{Name: "echo"},
			Required: []string{"text"},
			Handler: func(state *tools.State, args map[string]any) (any, error) {
				return filepathBase(state.Root) + ":" + args["text"].(string), nil
			},
		},
	}
	client := &stubCompletionClient{responses: []providers.ChatMessage{
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_1", "echo", map[string]any{"text": "hi"})}),
		providers.AssistantMessage("done", nil),
	}}
	printed := []string{}
	oldPrint := PrintFunc
	defer func() { PrintFunc = oldPrint }()
	PrintFunc = func(value string) { printed = append(printed, value) }

	root := t.TempDir()
	transcript := TranscriptWithSystemPrompt("sys")
	AddUser(&transcript, "hello")
	state := AgentState(root, registry, 3600, time.Now().Add(time.Hour), false, false, false, nil)
	code, content, err := RunTurn(client, &transcript, &state, "openai:gpt-test", tools.ToolSpecs(registry), 1)
	if err != nil || code != 0 || content != "done" {
		t.Fatalf("unexpected result: %d %q %v", code, content, err)
	}
	if len(printed) != 1 || printed[0] != "done" {
		t.Fatalf("unexpected printed output: %#v", printed)
	}
	if transcript.Messages[2].ToolCalls[0].Name != "echo" {
		t.Fatalf("unexpected assistant tool call: %#v", transcript.Messages[2])
	}
	if !strings.Contains(transcript.Messages[3].Content, filepathBase(root)+":hi") {
		t.Fatalf("unexpected tool message: %#v", transcript.Messages[3])
	}
}

func TestRunTurnPropagatesTodoStateToNextRequest(t *testing.T) {
	client := &stubCompletionClient{responses: []providers.ChatMessage{
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_1", "todo", map[string]any{"todos": []any{map[string]any{"id": "t1", "task": "ship it", "status": "in_progress"}}})}),
		providers.AssistantMessage("done", nil),
	}}
	oldPrint := PrintFunc
	defer func() { PrintFunc = oldPrint }()
	PrintFunc = func(string) {}
	transcript := TranscriptWithSystemPrompt("sys")
	AddUser(&transcript, "hello")
	state := AgentState(t.TempDir(), tools.ToolRegistry, 3600, time.Now().Add(time.Hour), false, false, false, nil)
	code, content, err := RunTurn(client, &transcript, &state, "openai:gpt-test", tools.ToolSpecs(tools.ToolRegistry), 1)
	if err != nil || code != 0 || content != "done" {
		t.Fatalf("unexpected result: %d %q %v", code, content, err)
	}
	if len(state.Todos) != 1 || state.Todos[0]["id"] != "t1" {
		t.Fatalf("unexpected propagated todos: %#v", state.Todos)
	}
	if len(client.requests) != 2 {
		t.Fatalf("unexpected request count: %d", len(client.requests))
	}
	found := false
	for _, message := range client.requests[1] {
		if message.Role == "system" && strings.Contains(message.Content, "t1: ship it") {
			found = true
			break
		}
	}
	if !found {
		t.Fatalf("expected todo state in next request: %#v", client.requests[1])
	}
}

func TestRunTurnPropagatesApprovalStateAcrossTurns(t *testing.T) {
	registry := map[string]tools.RegisteredTool{
		"mutating": {
			Spec:     tools.Spec{Name: "mutating", Mutating: true},
			Required: []string{"text"},
			Handler: func(state *tools.State, args map[string]any) (any, error) {
				return args["text"].(string), nil
			},
		},
	}
	client := &stubCompletionClient{responses: []providers.ChatMessage{
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_1", "mutating", map[string]any{"text": "first"})}),
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_2", "mutating", map[string]any{"text": "second"})}),
		providers.AssistantMessage("done", nil),
	}}
	prompts := 0
	oldApproval, oldPrint := tools.ApprovalPromptFunc, PrintFunc
	defer func() {
		tools.ApprovalPromptFunc = oldApproval
		PrintFunc = oldPrint
	}()
	tools.ApprovalPromptFunc = func(_ string, _ []string) string {
		prompts++
		return "all"
	}
	PrintFunc = func(string) {}
	transcript := TranscriptWithSystemPrompt("sys")
	AddUser(&transcript, "hello")
	state := AgentState(t.TempDir(), registry, 3600, time.Now().Add(time.Hour), true, false, false, nil)
	code, content, err := RunTurn(client, &transcript, &state, "openai:gpt-test", tools.ToolSpecs(registry), 1)
	if err != nil || code != 0 || content != "done" {
		t.Fatalf("unexpected result: %d %q %v", code, content, err)
	}
	if !state.ApproveAllMutatingTools || prompts != 1 {
		t.Fatalf("expected approval state to persist, got prompts=%d state=%#v", prompts, state)
	}
}

func TestSelfConsistencyLogsOnlyNonUnanimousChoice(t *testing.T) {
	client := &stubCompletionClient{responses: []providers.ChatMessage{
		providers.AssistantMessage("wrong", nil),
		providers.AssistantMessage("done", nil),
		providers.AssistantMessage("done", nil),
	}}
	notes := []string{}
	oldPrint, oldNote := PrintFunc, NoteFunc
	defer func() { PrintFunc, NoteFunc = oldPrint, oldNote }()
	PrintFunc = func(string) {}
	NoteFunc = func(message string) { notes = append(notes, message) }
	transcript := TranscriptWithSystemPrompt("sys")
	AddUser(&transcript, "hello")
	state := AgentState(t.TempDir(), map[string]tools.RegisteredTool{}, 3600, time.Now().Add(time.Hour), false, false, false, nil)
	code, content, err := RunTurn(client, &transcript, &state, "openai:gpt-test", tools.ToolSpecs(map[string]tools.RegisteredTool{}), 3)
	if err != nil || code != 0 || content != "done" {
		t.Fatalf("unexpected result: %d %q %v", code, content, err)
	}
	if len(notes) != 1 || notes[0] != "self-consistency: sample 2 won 2/3" {
		t.Fatalf("unexpected notes: %#v", notes)
	}
}

func TestChooseSelfConsistentMessagePrefersSimilarToolPlans(t *testing.T) {
	messages := []providers.ChatMessage{
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_1", "search", map[string]any{"pattern": "auth token", "path": "src"})}),
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_2", "search", map[string]any{"pattern": "token auth", "path": "src"})}),
		providers.AssistantMessage("", []providers.ToolCall{providers.ToolCallMessage("call_3", "read", map[string]any{"path": "README.md"})}),
	}
	chosen, index, votes, err := ChooseSelfConsistentMessage(messages)
	if err != nil {
		t.Fatal(err)
	}
	if chosen.ToolCalls[0].Name != "search" || index != 0 || votes != 2 {
		t.Fatalf("unexpected chosen message: %#v %d %d", chosen, index, votes)
	}
}

func filepathBase(path string) string {
	parts := strings.Split(strings.TrimRight(path, "/"), "/")
	return parts[len(parts)-1]
}
