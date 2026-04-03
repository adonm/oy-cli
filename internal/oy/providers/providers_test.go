package providers

import "testing"

func TestSplitJoinModelSpec(t *testing.T) {
	shim, model := SplitModelSpec("openai:gpt-test")
	if shim != "openai" || model != "gpt-test" {
		t.Fatalf("unexpected split: %q %q", shim, model)
	}
	if got := JoinModelSpec("copilot", "gpt-5"); got != "copilot:gpt-5" {
		t.Fatalf("unexpected join: %q", got)
	}
}
