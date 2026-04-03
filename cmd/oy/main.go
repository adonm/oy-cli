package main

import (
	"os"

	"github.com/wagov-dtt/oy-cli/internal/oy/cli"
)

func main() {
	os.Exit(cli.Main(os.Args[1:]))
}
