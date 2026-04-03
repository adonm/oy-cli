package tools

import "github.com/wagov-dtt/oy-cli/internal/oy/runtime"

type Spec struct {
	Name        string         `json:"name"`
	Description string         `json:"description"`
	Parameters  map[string]any `json:"parameters"`
	Mutating    bool           `json:"mutating,omitempty"`
}

func DefaultListLimit() int {
	return runtime.DefaultBudgets.DefaultLineLimit
}
