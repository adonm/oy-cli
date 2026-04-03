package ui

import (
	"io"
	"strings"
	"testing"

	"github.com/chzyer/readline"
)

type stubLineEditor struct {
	closeCount int
	lines      []string
	prompt     string
	errs       []error
}

func (s *stubLineEditor) Close() error {
	s.closeCount++
	return nil
}

func (s *stubLineEditor) Readline() (string, error) {
	line := ""
	if len(s.lines) > 0 {
		line = s.lines[0]
		s.lines = s.lines[1:]
	}
	var err error
	if len(s.errs) > 0 {
		err = s.errs[0]
		s.errs = s.errs[1:]
	}
	return line, err
}

func (s *stubLineEditor) SetPrompt(prompt string) { s.prompt = prompt }

func TestPromptIOLineUsesInteractiveLineEditor(t *testing.T) {
	old := newLineEditorFunc
	defer func() { newLineEditorFunc = old }()

	stub := &stubLineEditor{lines: []string{"  hello  ", "world"}}
	calls := 0
	newLineEditorFunc = func(input io.Reader, output io.Writer) (lineEditor, error) {
		calls++
		return stub, nil
	}

	prompt := NewPromptIO(strings.NewReader("ignored"), io.Discard, true)
	got, err := prompt.Line("oy >")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got != "hello" {
		t.Fatalf("unexpected line: %q", got)
	}
	if stub.prompt != "oy > " {
		t.Fatalf("unexpected prompt: %q", stub.prompt)
	}
	got, err = prompt.Line("oy >")
	if err != nil {
		t.Fatalf("unexpected second error: %v", err)
	}
	if got != "world" {
		t.Fatalf("unexpected second line: %q", got)
	}
	if calls != 1 {
		t.Fatalf("expected one line editor init, got %d", calls)
	}
}

func TestPromptIOLineReturnsEOFOnInterrupt(t *testing.T) {
	old := newLineEditorFunc
	defer func() { newLineEditorFunc = old }()

	newLineEditorFunc = func(input io.Reader, output io.Writer) (lineEditor, error) {
		return &stubLineEditor{lines: []string{"partial"}, errs: []error{readline.ErrInterrupt}}, nil
	}

	prompt := NewPromptIO(strings.NewReader("ignored"), io.Discard, true)
	got, err := prompt.Line("oy >")
	if err != io.EOF {
		t.Fatalf("expected EOF, got %v", err)
	}
	if got != "" {
		t.Fatalf("expected empty line on interrupt, got %q", got)
	}
}

func TestPromptIOCloseClosesInteractiveLineEditor(t *testing.T) {
	old := newLineEditorFunc
	defer func() { newLineEditorFunc = old }()

	stub := &stubLineEditor{}
	newLineEditorFunc = func(input io.Reader, output io.Writer) (lineEditor, error) {
		return stub, nil
	}

	prompt := NewPromptIO(strings.NewReader("ignored"), io.Discard, true)
	if prompt.lineEditor() == nil {
		t.Fatal("expected interactive line editor")
	}
	if err := prompt.Close(); err != nil {
		t.Fatalf("unexpected close error: %v", err)
	}
	if stub.closeCount != 1 {
		t.Fatalf("expected one close, got %d", stub.closeCount)
	}
}

func TestPromptIOLineAccessibleFallback(t *testing.T) {
	prompt := NewPromptIO(strings.NewReader("  hi  \n"), io.Discard, false)
	got, err := prompt.Line("oy >")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if got != "hi" {
		t.Fatalf("unexpected line: %q", got)
	}
}
