package providers

import (
	"fmt"
	"os"
	"strings"
)

type ShimSpec struct {
	EnsureEnv   func(cwd string) error
	BuildClient func(cwd string) (CompletionClient, error)
	ListModels  func(cwd string) ([]string, error)
}

type noopClient struct{}

func (noopClient) ChatCompletion(string, []ChatMessage, []map[string]any, string) (ChatMessage, error) {
	return ChatMessage{}, fmt.Errorf("not implemented")
}
func (noopClient) ListModels() ([]string, error) { return nil, fmt.Errorf("not implemented") }

var ShimSpecs = map[string]ShimSpec{
	ShimOpenAI: {
		EnsureEnv: func(string) error {
			if strings.TrimSpace(os.Getenv("OPENAI_API_KEY")) == "" {
				return fmt.Errorf("OPENAI_API_KEY is not set")
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) { return noopClient{}, nil },
		ListModels:  func(string) ([]string, error) { return nil, nil },
	},
	ShimCodex: {
		EnsureEnv:   func(string) error { _, err := LoadCodexSession(); return err },
		BuildClient: func(string) (CompletionClient, error) { return noopClient{}, nil },
		ListModels:  func(string) ([]string, error) { return nil, nil },
	},
	ShimMantle: {
		EnsureEnv:   func(string) error { return nil },
		BuildClient: func(string) (CompletionClient, error) { return noopClient{}, nil },
		ListModels:  func(string) ([]string, error) { return nil, nil },
	},
	ShimCopilot: {
		EnsureEnv: func(string) error {
			if GetGitHubToken() == "" {
				return fmt.Errorf("No GitHub token found (set GH_TOKEN, GITHUB_TOKEN, or run `gh auth login`)")
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) { return noopClient{}, nil },
		ListModels:  func(string) ([]string, error) { return nil, nil },
	},
	ShimOpenCode: {
		EnsureEnv: func(string) error {
			if OpencodeAPIKey("opencode") == "" {
				return fmt.Errorf("No OpenCode Zen credentials found in %s (run `opencode auth`)", OpencodeAuthPath)
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) { return noopClient{}, nil },
		ListModels:  func(string) ([]string, error) { return nil, nil },
	},
	ShimOpenCodeGo: {
		EnsureEnv: func(string) error {
			if OpencodeAPIKey("opencode-go") == "" {
				return fmt.Errorf("No OpenCode Go credentials found in %s (run `opencode auth`)", OpencodeAuthPath)
			}
			return nil
		},
		BuildClient: func(string) (CompletionClient, error) { return noopClient{}, nil },
		ListModels:  func(string) ([]string, error) { return nil, nil },
	},
}

func shimSpec(shim string) (ShimSpec, error) {
	if err := ValidateShim(shim); err != nil {
		return ShimSpec{}, err
	}
	spec, ok := ShimSpecs[shim]
	if !ok {
		return ShimSpec{}, fmt.Errorf("missing shim spec for %s", shim)
	}
	return spec, nil
}

func shimEnvError(spec ShimSpec, cwd string) string {
	if spec.EnsureEnv == nil {
		return ""
	}
	if err := spec.EnsureEnv(cwd); err != nil {
		return err.Error()
	}
	return ""
}

func shimAvailable(shim string) bool {
	spec, err := shimSpec(shim)
	if err != nil {
		return false
	}
	return shimEnvError(spec, "") == ""
}

func DetectAvailableShims() []string {
	items := make([]string, 0, len(ShimOrder))
	for _, shim := range ShimOrder {
		if shimAvailable(shim) {
			items = append(items, shim)
		}
	}
	return items
}

func ResolveShim(modelSpec, configuredShim string) string {
	if envShim := strings.TrimSpace(os.Getenv("OY_SHIM")); envShim != "" {
		return envShim
	}
	if prefix, _ := SplitModelSpec(modelSpec); prefix != "" {
		return prefix
	}
	if configuredShim != "" {
		return configuredShim
	}
	available := DetectAvailableShims()
	if len(available) > 0 {
		return available[0]
	}
	return ShimOpenAI
}

func EnsureAPIEnv(modelSpec, configuredShim, cwd string) (bool, string) {
	spec, err := shimSpec(ResolveShim(modelSpec, configuredShim))
	if err != nil {
		return false, err.Error()
	}
	if msg := shimEnvError(spec, cwd); msg != "" {
		return false, msg
	}
	return true, ""
}

func RequireAPIEnv(modelSpec, configuredShim, cwd string) (string, error) {
	shim := ResolveShim(modelSpec, configuredShim)
	spec, err := shimSpec(shim)
	if err != nil {
		return "", err
	}
	if msg := shimEnvError(spec, cwd); msg != "" {
		return "", fmt.Errorf("missing API credentials\n\n- error: %s", msg)
	}
	return shim, nil
}

func GetClient(shim, cwd string) (CompletionClient, error) {
	spec, err := shimSpec(shim)
	if err != nil {
		return nil, err
	}
	if spec.BuildClient == nil {
		return nil, fmt.Errorf("shim %s does not implement a client", shim)
	}
	return spec.BuildClient(cwd)
}

func ListModelsForShim(shim, cwd string, ignoreErrors bool) ([]string, error) {
	spec, err := shimSpec(shim)
	if err != nil {
		return nil, err
	}
	if spec.ListModels == nil {
		return nil, nil
	}
	items, err := spec.ListModels(cwd)
	if err != nil {
		if ignoreErrors {
			return []string{}, nil
		}
		return nil, err
	}
	prefixed := make([]string, 0, len(items))
	for _, item := range items {
		prefixed = append(prefixed, JoinModelSpec(shim, item))
	}
	return prefixed, nil
}
