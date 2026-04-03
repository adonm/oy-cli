package runtime

import "testing"

func TestDefaultBestOfForModel(t *testing.T) {
	for _, model := range []string{"openai:glm-5", "bedrock-mantle:moonshotai.kimi-k2.5"} {
		if got := DefaultBestOfForModel(model); got != DefaultSelfConsistencyBestOf {
			t.Fatalf("unexpected best-of for %q: %d", model, got)
		}
	}
	if got := DefaultBestOfForModel("openai:gpt-5"); got != 1 {
		t.Fatalf("unexpected default for gpt-5: %d", got)
	}
}

func TestResolvePathDeniesTraversal(t *testing.T) {
	root := "/tmp/work"
	if _, err := ResolvePath(root, "ok/file.txt"); err != nil {
		t.Fatalf("expected inside path, got error: %v", err)
	}
	if _, err := ResolvePath(root, "../etc/passwd"); err == nil {
		t.Fatal("expected traversal error")
	}
}
