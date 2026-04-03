package tools

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
)

type stubHTTPClient struct {
	response providers.ResponseAdapter
	err      error
}

func (s stubHTTPClient) Request(method, url string, headers map[string]string, body []byte) (providers.ResponseAdapter, error) {
	_ = method
	_ = url
	_ = headers
	_ = body
	return s.response, s.err
}

func makeState(root string) State {
	return State{Root: root}
}

func TestToolSpecsClosedObjectSchemas(t *testing.T) {
	specs := map[string]map[string]any{}
	for _, tool := range ToolSpecs(nil) {
		specs[tool["name"].(string)] = tool
	}
	if specs["todo"]["parameters"].(map[string]any)["additionalProperties"] != false {
		t.Fatal("todo schema should be a closed object")
	}
	if specs["todo"]["parameters"].(map[string]any)["properties"].(map[string]any)["todos"].(map[string]any)["items"].(map[string]any)["additionalProperties"] != false {
		t.Fatal("todo items should be closed objects")
	}
	for _, name := range []string{"list", "search", "replace", "sloc"} {
		properties := specs[name]["parameters"].(map[string]any)["properties"].(map[string]any)
		if _, ok := properties["exclude"]; !ok {
			t.Fatalf("schema missing exclude: %s", name)
		}
	}
}

func TestInvokeToolApprovalAndValidation(t *testing.T) {
	calls := []string{}
	registry := map[string]RegisteredTool{
		"mutating": {
			Spec:     Spec{Name: "mutating", Mutating: true},
			Required: []string{"text"},
			Handler: func(_ *State, args map[string]any) (any, error) {
				calls = append(calls, args["text"].(string))
				return "done:" + args["text"].(string), nil
			},
		},
	}
	oldApproval := ApprovalPromptFunc
	defer func() { ApprovalPromptFunc = oldApproval }()

	denied := &State{Interactive: true}
	ApprovalPromptFunc = func(_ string, _ []string) string { return "deny" }
	result := InvokeTool(registry, denied, "mutating", map[string]any{"text": "nope"})
	if result.OK || result.Content.(map[string]any)["error_type"] != "PermissionError" || len(calls) != 0 {
		t.Fatalf("unexpected denied result: %#v %#v", result, calls)
	}

	approved := &State{Interactive: true}
	ApprovalPromptFunc = func(_ string, _ []string) string { return "all" }
	first := InvokeTool(registry, approved, "mutating", map[string]any{"text": "first"})
	second := InvokeTool(registry, approved, "mutating", map[string]any{"text": "second"})
	if !first.OK || !second.OK || !approved.ApproveAllMutatingTools || strings.Join(calls, ",") != "first,second" {
		t.Fatalf("unexpected approved results/state: %#v %#v %#v", first, second, approved)
	}

	invalid := InvokeTool(registry, &State{}, "mutating", map[string]any{})
	if invalid.OK || invalid.Content.(map[string]any)["error_type"] != "ValidationError" {
		t.Fatalf("unexpected invalid result: %#v", invalid)
	}
}

func TestAskAndTodoUpdateState(t *testing.T) {
	state := State{Root: t.TempDir(), Interactive: true}
	oldAsk, oldSelect := AskInputFunc, SelectInputFunc
	defer func() {
		AskInputFunc, SelectInputFunc = oldAsk, oldSelect
	}()
	AskInputFunc = func(_ string) string { return " free " }
	SelectInputFunc = func(_ string, _ []string) string { return "beta" }
	if answer, err := ToolAsk(&state, "Question?", nil); err != nil || answer != "free" {
		t.Fatalf("unexpected free-form ask result: %q %v", answer, err)
	}
	if answer, err := ToolAsk(&state, "Choose", []string{"alpha", "beta"}); err != nil || answer != "beta" {
		t.Fatalf("unexpected choice ask result: %q %v", answer, err)
	}
	payload, err := ToolTodo(&state, []map[string]string{{"id": "t1", "task": "ship it", "status": "in_progress"}})
	if err != nil || len(state.Todos) != 1 || payload["count"].(int) != 1 {
		t.Fatalf("unexpected todo payload/state: %#v %#v %v", payload, state.Todos, err)
	}
	if TodoPreview(state.Todos) != "count: 1\ntodos:\n  [~] t1: ship it" {
		t.Fatalf("unexpected todo preview: %q", TodoPreview(state.Todos))
	}
	if _, err := ToolTodo(&state, []map[string]string{{"id": "t2", "task": "bad", "status": "wat"}}); err == nil {
		t.Fatal("expected invalid todo status error")
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
	if err := os.WriteFile(filepath.Join(root, "main.go"), []byte("package main\n\nfunc main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state := makeState(root)

	listPayload, err := ToolList(state, "*", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	items := listPayload["items"].([]string)
	if len(items) < 2 || !containsString(items, "a.txt") || !containsString(items, "dir/") {
		t.Fatalf("unexpected list payload: %#v", listPayload)
	}

	listDotPayload, err := ToolList(state, ".", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	if !containsString(listDotPayload["items"].([]string), "a.txt") || !containsString(listDotPayload["items"].([]string), "dir/") {
		t.Fatalf("unexpected dot list payload: %#v", listDotPayload)
	}

	listNestedPayload, err := ToolList(state, "**/*.py", nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	if got := listNestedPayload["items"].([]string); len(got) != 1 || got[0] != "dir/b.py" {
		t.Fatalf("unexpected nested list payload: %#v", listNestedPayload)
	}

	listExcludedPayload, err := ToolList(state, "*", []string{"dir/**"}, 20)
	if err != nil {
		t.Fatal(err)
	}
	if containsString(listExcludedPayload["items"].([]string), "dir/") {
		t.Fatalf("expected dir exclusion: %#v", listExcludedPayload)
	}
	if _, err := ToolList(state, "../*", nil, 20); err == nil {
		t.Fatal("expected traversal error for list")
	}

	readPayload, preview, err := ToolRead(state, "a.txt", 2, 1)
	if err != nil {
		t.Fatal(err)
	}
	if readPayload["text"].(string) != "beta" || !strings.Contains(preview, "text: beta") {
		t.Fatalf("unexpected read payload/preview: %#v %q", readPayload, preview)
	}

	searchPayload, err := ToolSearch(state, "alpha|hello", ".", "", false, false, nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	if searchPayload["match_count"].(int) != 2 {
		t.Fatalf("unexpected search payload: %#v", searchPayload)
	}

	fuzzyPayload, err := ToolSearch(state, "hello", ".", "s<=1", true, false, nil, 20)
	if err != nil {
		t.Fatal(err)
	}
	fuzzyMatches := fuzzyPayload["matches"].([]map[string]any)
	if len(fuzzyMatches) == 0 || fuzzyMatches[0]["path"].(string) != "dir/b.py" || fuzzyMatches[0]["column"].(int) != 8 || fuzzyPayload["fuzzy"].(string) != "s<=1" || fuzzyPayload["best_match"].(bool) != true {
		t.Fatalf("unexpected fuzzy search payload: %#v", fuzzyPayload)
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
	if slocPayload["language_count"].(int) < 3 {
		t.Fatalf("expected multiple languages in sloc payload: %#v", slocPayload)
	}
	languages := slocPayload["languages"].([]map[string]any)
	if !containsLanguage(languages, "Python") || !containsLanguage(languages, "Go") || !containsLanguage(languages, "Text") {
		t.Fatalf("unexpected sloc languages: %#v", languages)
	}
}

func TestSlocTopFilesTruncateAtTwenty(t *testing.T) {
	root := t.TempDir()
	for i := 0; i < 25; i++ {
		lines := make([]string, 0, i+1)
		for line := 0; line <= i; line++ {
			lines = append(lines, "v = 1")
		}
		if err := os.WriteFile(filepath.Join(root, fmt.Sprintf("file_%02d.py", i)), []byte(strings.Join(lines, "\n")+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	payload, err := ToolSloc(makeState(root), ".", nil, 3)
	if err != nil {
		t.Fatal(err)
	}
	if payload["top_file_count"].(int) != 25 || len(payload["top_files"].([]map[string]any)) != 20 || payload["truncated"].(bool) != true {
		t.Fatalf("unexpected truncated sloc payload: %#v", payload)
	}
}

func TestValidateURLSafe(t *testing.T) {
	for _, raw := range []string{"file:///etc/passwd", "http://localhost/secret", "http://127.0.0.1/secret", "http://169.254.169.254/latest/meta-data/"} {
		if err := ValidateURLSafe(raw); err == nil {
			t.Fatalf("expected validation error for %q", raw)
		}
	}
}

func TestWebfetchHTMLAndRedaction(t *testing.T) {
	oldFactory := ToolSessionFactory
	defer func() { ToolSessionFactory = oldFactory }()
	htmlBody := "<html><body><h1>Title</h1><p>Paragraph 1 with <a href='/doc/1'>link 1</a>.</p></body></html>"
	ToolSessionFactory = func(timeout time.Duration, followRedirects bool) HTTPRequester {
		_ = timeout
		_ = followRedirects
		return stubHTTPClient{response: providers.ResponseAdapter{StatusCode: 200, Headers: map[string]string{"content-type": "text/html; charset=utf-8", "location": "https://secret.example/next", "set-cookie": "session=secret"}, Text: htmlBody, Content: []byte(htmlBody), URL: "https://example.com/page", ReasonPhrase: "OK", HTTPVersion: "HTTP/1.1"}}
	}
	payload, err := ToolWebfetch(makeState(t.TempDir()), "https://example.com/page", "GET", nil, false, 60)
	if err != nil {
		t.Fatal(err)
	}
	headers := payload["headers"].(map[string]string)
	if payload["format"].(string) != "markdown" || !strings.Contains(payload["text"].(string), "Title\n=====") || headers["Location"] != "<redacted>" || headers["Set-Cookie"] != "<redacted>" {
		t.Fatalf("unexpected webfetch payload: %#v", payload)
	}
}

func TestWebfetchErrorPayloadAndRestrictions(t *testing.T) {
	oldFactory := ToolSessionFactory
	defer func() { ToolSessionFactory = oldFactory }()
	ToolSessionFactory = func(timeout time.Duration, followRedirects bool) HTTPRequester {
		_ = timeout
		_ = followRedirects
		return stubHTTPClient{err: &providers.TransportError{Message: "boom"}}
	}
	payload, err := ToolWebfetch(makeState(t.TempDir()), "https://example.com/page", "GET", nil, false, 60)
	if err != nil {
		t.Fatal(err)
	}
	if payload["ok"].(bool) != false || payload["error_type"].(string) != "TransportError" {
		t.Fatalf("unexpected error payload: %#v", payload)
	}
	if _, err := ToolWebfetch(makeState(t.TempDir()), "https://example.com/page", "POST", nil, false, 60); err == nil {
		t.Fatal("expected invalid method error")
	}
	if _, err := ToolWebfetch(makeState(t.TempDir()), "https://example.com/page", "GET", map[string]string{"Authorization": "secret"}, false, 60); err == nil {
		t.Fatal("expected invalid header error")
	}
}

func TestRegistryHelpers(t *testing.T) {
	active := ActiveToolRegistry(false)
	if _, ok := active["ask"]; ok {
		t.Fatal("ask should be removed from non-interactive registry")
	}
	readOnly := ReadOnlyToolRegistry()
	for _, name := range []string{"list", "read", "search", "sloc", "webfetch"} {
		if _, ok := readOnly[name]; !ok {
			t.Fatalf("missing read-only tool: %s", name)
		}
	}
	if result := InvokeTool(map[string]RegisteredTool{}, &State{}, "missing", nil); result.OK || result.Content.(string) != "Tool 'missing' unavailable" {
		t.Fatalf("unexpected missing-tool result: %#v", result)
	}
}

func containsLanguage(items []map[string]any, want string) bool {
	for _, item := range items {
		if item["language"] == want {
			return true
		}
	}
	return false
}

func containsString(items []string, want string) bool {
	for _, item := range items {
		if item == want {
			return true
		}
	}
	return false
}

var _ = errors.New
