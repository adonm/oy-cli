package tools

import (
	"fmt"
	"strings"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
)

func BashPayload(command string, result providers.CommandResult) (map[string]any, string) {
	payload := map[string]any{
		"command":    command,
		"returncode": result.ReturnCode,
		"stdout":     strings.TrimSuffix(result.Stdout, "\n"),
		"stderr":     strings.TrimSuffix(result.Stderr, "\n"),
	}
	preview := fmt.Sprintf("$ %s\nexit: %d\nstdout:\n%s", command, result.ReturnCode, payload["stdout"])
	if payload["stderr"] != "" {
		preview += fmt.Sprintf("\nstderr:\n%s", payload["stderr"])
	}
	return payload, preview
}

func ToolBash(state State, command string, timeoutSeconds int) (map[string]any, string, error) {
	if len([]byte(command)) > runtime.MaxBashCmdBytes() {
		return nil, "", fmt.Errorf("command too large (%d chars); limit is %d bytes", len(command), runtime.MaxBashCmdBytes())
	}
	env := providers.CommandEnv(state.Root)
	bashPath := providers.Which("bash", env["PATH"])
	if bashPath == "" {
		return nil, "", fmt.Errorf("bash is not installed or not on PATH")
	}
	result, err := providers.RunCmd([]string{bashPath, "-c", command}, state.Root, env, timeDurationSeconds(timeoutSeconds), "")
	if err != nil {
		return nil, "", err
	}
	payload, preview := BashPayload(command, result)
	return payload, preview, nil
}
