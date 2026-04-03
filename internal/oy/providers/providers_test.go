package providers

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

type stubClient struct {
	models []string
}

func (s stubClient) ChatCompletion(string, []ChatMessage, []map[string]any, string) (ChatMessage, error) {
	return ChatMessage{}, nil
}
func (s stubClient) ListModels() ([]string, error) { return s.models, nil }

func TestSplitJoinModelSpec(t *testing.T) {
	shim, model := SplitModelSpec("openai:gpt-test")
	if shim != "openai" || model != "gpt-test" {
		t.Fatalf("unexpected split: %q %q", shim, model)
	}
	if got := JoinModelSpec("copilot", "gpt-5"); got != "copilot:gpt-5" {
		t.Fatalf("unexpected join: %q", got)
	}
}

func TestAuthLoadersIgnoreNonObjectJSON(t *testing.T) {
	tmp := t.TempDir()
	codex := filepath.Join(tmp, "codex.json")
	opencode := filepath.Join(tmp, "opencode.json")
	if err := os.WriteFile(codex, []byte("[]"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(opencode, []byte("[]"), 0o600); err != nil {
		t.Fatal(err)
	}
	oldCodex, oldOpenCode := CodexAuthPath, OpencodeAuthPath
	CodexAuthPath, OpencodeAuthPath = codex, opencode
	defer func() {
		CodexAuthPath, OpencodeAuthPath = oldCodex, oldOpenCode
	}()
	if got := LoadCodexAuth(); !reflect.DeepEqual(got, map[string]any{}) {
		t.Fatalf("unexpected codex auth: %#v", got)
	}
	if got := OpencodeAPIKey("opencode"); got != "" {
		t.Fatalf("unexpected opencode key: %q", got)
	}
	if got := OpencodeAPIKey("opencode-go"); got != "" {
		t.Fatalf("unexpected opencode-go key: %q", got)
	}
}

func TestResponseRaiseForStatus(t *testing.T) {
	response := ResponseAdapter{StatusCode: 401, Text: `{"error":{"message":"bad token"}}`, ReasonPhrase: "Unauthorized"}
	err := ResponseRaiseForStatus(response)
	if _, ok := err.(*AuthenticationError); !ok {
		t.Fatalf("unexpected error type: %T", err)
	}
	if err.Error() != "bad token" {
		t.Fatalf("unexpected error message: %v", err)
	}
}

func TestShimRegistryOrderAndClientBuilding(t *testing.T) {
	calls := []string{}
	sentinel := stubClient{models: []string{"demo"}}
	oldOrder, oldKnown, oldSpecs := ShimOrder, KnownShims, ShimSpecs
	defer func() {
		ShimOrder, KnownShims, ShimSpecs = oldOrder, oldKnown, oldSpecs
	}()
	okEnsure := func(name string) func(string) error {
		return func(cwd string) error {
			calls = append(calls, name+":"+cwd)
			return nil
		}
	}
	failEnsure := func(name string) func(string) error {
		return func(cwd string) error {
			calls = append(calls, name+":"+cwd)
			return assertErr(name)
		}
	}
	ShimOrder = []string{"alpha", "beta", "gamma"}
	KnownShims = map[string]struct{}{"alpha": {}, "beta": {}, "gamma": {}}
	ShimSpecs = map[string]ShimSpec{
		"alpha": {EnsureEnv: okEnsure("alpha"), BuildClient: func(string) (CompletionClient, error) { return sentinel, nil }, ListModels: func(string) ([]string, error) { return []string{"demo"}, nil }},
		"beta":  {EnsureEnv: failEnsure("beta"), BuildClient: func(string) (CompletionClient, error) { return nil, nil }, ListModels: func(string) ([]string, error) { return nil, nil }},
		"gamma": {EnsureEnv: okEnsure("gamma"), BuildClient: func(string) (CompletionClient, error) { return nil, nil }, ListModels: func(string) ([]string, error) { return []string{}, nil }},
	}
	available := DetectAvailableShims()
	if !reflect.DeepEqual(available, []string{"alpha", "gamma"}) {
		t.Fatalf("unexpected available shims: %#v", available)
	}
	if !reflect.DeepEqual(calls, []string{"alpha:", "beta:", "gamma:"}) {
		t.Fatalf("unexpected ensure calls: %#v", calls)
	}
	ok, msg := EnsureAPIEnv("alpha:model", "", "/tmp/work")
	if !ok || msg != "" {
		t.Fatalf("unexpected ensure result: %v %q", ok, msg)
	}
	client, err := GetClient("alpha", "/tmp/work")
	if err != nil {
		t.Fatal(err)
	}
	models, err := client.ListModels()
	if err != nil || !reflect.DeepEqual(models, []string{"demo"}) {
		t.Fatalf("unexpected models: %#v %v", models, err)
	}
	prefixed, err := ListModelsForShim("alpha", "/tmp/work", true)
	if err != nil || !reflect.DeepEqual(prefixed, []string{"alpha:demo"}) {
		t.Fatalf("unexpected prefixed models: %#v %v", prefixed, err)
	}
}

func TestListModelsErrorHandling(t *testing.T) {
	oldOrder, oldKnown, oldSpecs := ShimOrder, KnownShims, ShimSpecs
	defer func() {
		ShimOrder, KnownShims, ShimSpecs = oldOrder, oldKnown, oldSpecs
	}()
	ShimOrder = []string{"gamma"}
	KnownShims = map[string]struct{}{"gamma": {}}
	ShimSpecs = map[string]ShimSpec{
		"gamma": {EnsureEnv: func(string) error { return nil }, BuildClient: func(string) (CompletionClient, error) { return nil, nil }, ListModels: func(string) ([]string, error) { return nil, assertErr("boom") }},
	}
	items, err := ListModelsForShim("gamma", "/tmp/work", true)
	if err != nil || len(items) != 0 {
		t.Fatalf("unexpected ignore_errors result: %#v %v", items, err)
	}
	if _, err := ListModelsForShim("gamma", "/tmp/work", false); err == nil {
		t.Fatal("expected error")
	}
}

func assertErr(message string) error { return &simpleErr{message: message} }

type simpleErr struct{ message string }

func (e *simpleErr) Error() string { return e.message }
