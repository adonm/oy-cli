package cli

import (
	"fmt"
	"os"

	"github.com/wagov-dtt/oy-cli/internal/oy/version"
)

// Main is the top-level CLI entrypoint for the Go port.
func Main(argv []string) int {
	if len(argv) > 0 {
		switch argv[0] {
		case "-v", "--version":
			fmt.Printf("oy %s\n", version.Version)
			return 0
		case "-h", "--help":
			printHelp()
			return 0
		}
	}
	printHelp()
	return 0
}

func printHelp() {
	_, _ = fmt.Fprintln(os.Stdout, "oy (Go port in progress)")
	_, _ = fmt.Fprintln(os.Stdout, "")
	_, _ = fmt.Fprintln(os.Stdout, "Commands: run, chat, ralph, model, audit")
	_, _ = fmt.Fprintln(os.Stdout, "Progress is tracked in GO_PORT_TRACKER.md")
}
