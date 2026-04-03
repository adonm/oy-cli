package tools

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

func makeState(root string) State {
	return State{Root: root}
}

func TestTodoUpdatesState(t *testing.T) {
	state := makeState(t.TempDir())
	payload, err := ToolTodo(&state, []map[string]string{{"id": "t1", "task": "ship it", "status": "in_progress"}})
	if err != nil {
		t.Fatal(err)
	}
	if len(state.Todos) != 1 || payload["count"].(int) != 1 {
		t.Fatalf("unexpected todo payload/state: %#v %#v", payload, state.Todos)
	}
	if TodoPreview(state.Todos) != "count: 1\ntodos:\n  [~] t1: ship it" {
		t.Fatalf("unexpected todo preview: %q", TodoPreview(state.Todos))
	}
}

func TestBashPayload(t *testing.T) {
	payload, preview := BashPayload("echo hi", providers.CommandResult{ReturnCode: 1, Stdout: "line1\nline2\n", Stderr: "boom\n"})
	if payload["returncode"].(int) != 1 {
		t.Fatalf("unexpected payload: %#v", payload)
	}
	if !strings.Contains(preview, "$ echo hi") || !strings.Contains(preview, "exit: 1") {
		t.Fatalf("unexpected preview: %q", preview)
	}
}

func TestListReadSearchReplaceSloc(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "a.txt"), []byte("alpha\nbeta\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(root, "dir"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "dir", "b.py"), []byte("print('hello')\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state := makeState(root)

	listPayload, err := ToolList(state, "*", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	items := listPayload["items"].([]string)
	if len(items) < 2 {
		t.Fatalf("unexpected list payload: %#v", listPayload)
	}

	readPayload, preview, err := ToolRead(state, "a.txt", 2, 1)
	if err != nil {
		t.Fatal(err)
	}
	if readPayload["text"].(string) != "beta" || !strings.Contains(preview, "text: beta") {
		t.Fatalf("unexpected read payload/preview: %#v %q", readPayload, preview)
	}

	searchPayload, err := ToolSearch(state, "alpha|hello", ".", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	if searchPayload["match_count"].(int) != 2 {
		t.Fatalf("unexpected search payload: %#v", searchPayload)
	}

	replacePayload, err := ToolReplace(state, "alpha", "ALPHA", ".", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	if replacePayload["changed_file_count"].(int) != 1 {
		t.Fatalf("unexpected replace payload: %#v", replacePayload)
	}

	slocPayload, err := ToolSloc(state, ".", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	if slocPayload["total_code_count"].(int) <= 0 {
		t.Fatalf("unexpected sloc payload: %#v", slocPayload)
	}
}

func TestValidateURLSafe(t *testing.T) {
	for _, raw := range []string{"file:///etc/passwd", "http://localhost/secret", "http://127.0.0.1/secret", "http://169.254.169.254/latest/meta-data/"} {
		if err := ValidateURLSafe(raw); err == nil {
			t.Fatalf("expected validation error for %q", raw)
		}
	}
}

func TestWebfetchPayloadRedaction(t *testing.T) {
	payload := WebfetchPayload(providers.ResponseAdapter{URL: "https://example.com/page", StatusCode: 200, ReasonPhrase: "OK", HTTPVersion: "HTTP/1.1", Headers: map[string]string{"content-type": "text/html", "location": "https://secret.example/next", "set-cookie": "session=secret"}}, "GET", ptr("Title\n=====\n"), true, "markdown")
	headers := payload["headers"].(map[string]string)
	if payload["format"].(string) != "markdown" || headers["Location"] != "<redacted>" || headers["Set-Cookie"] != "<redacted>" {
		t.Fatalf("unexpected payload: %#v", payload)
	}
}

func ptr(value string) *string { return &value }
