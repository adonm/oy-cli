package agent

import (
	"testing"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

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
