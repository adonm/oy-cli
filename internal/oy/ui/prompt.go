package ui

import (
	"bufio"
	"errors"
	"io"
	"os"
	"strings"
	"sync"

	"charm.land/huh/v2"
	"github.com/chzyer/readline"
)

type PromptIO struct {
	In         io.Reader
	Out        io.Writer
	Accessible bool
	state      *promptState
}

type Option struct {
	Key      string
	Value    string
	Selected bool
}

type lineEditor interface {
	Close() error
	Readline() (string, error)
	SetPrompt(string)
}

var newLineEditorFunc = newInteractiveLineEditor

type promptState struct {
	reader         *bufio.Reader
	lineEditor     lineEditor
	lineEditorOnce sync.Once
}

type nopReadCloser struct{ io.Reader }

func (nopReadCloser) Close() error { return nil }

func NewPromptIO(in io.Reader, out io.Writer, interactive bool) PromptIO {
	if in == nil {
		in = os.Stdin
	}
	if out == nil {
		out = os.Stderr
	}
	return PromptIO{In: in, Out: out, Accessible: !interactive, state: newPromptState(in)}
}

func InteractiveIO(in io.Reader, out io.Writer) bool {
	return isTTY(in) && isTTY(out)
}

func (p PromptIO) Line(title string) (string, error) {
	if editor := p.lineEditor(); editor != nil {
		editor.SetPrompt(strings.TrimRight(title, " ") + " ")
		value, err := editor.Readline()
		value = strings.TrimSpace(value)
		switch {
		case err == nil:
			return value, nil
		case errors.Is(err, io.EOF), errors.Is(err, readline.ErrInterrupt):
			return "", io.EOF
		default:
			return "", err
		}
	}
	value := ""
	field := huh.NewInput().Title(title).Prompt("").Inline(true).Value(&value)
	hitEOF, err := p.run(field)
	if err != nil {
		return "", err
	}
	value = strings.TrimSpace(value)
	if hitEOF && value == "" {
		return "", io.EOF
	}
	return value, nil
}

func (p PromptIO) Input(title, description, defaultValue string) (string, error) {
	value := defaultValue
	field := huh.NewInput().Title(title).Description(description).Prompt("").Value(&value)
	_, err := p.run(field)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(value), nil
}

func (p PromptIO) Confirm(title, description string, defaultYes bool, affirmative, negative string) (bool, error) {
	value := defaultYes
	field := huh.NewConfirm().Title(title).Description(description).Affirmative(affirmative).Negative(negative).Value(&value)
	_, err := p.run(field)
	if err != nil {
		return false, err
	}
	return value, nil
}

func (p PromptIO) Select(title, description string, options []Option, value string, filtering bool) (string, error) {
	selected := value
	field := huh.NewSelect[string]().Title(title).Description(description).Value(&selected).Filtering(filtering)
	if len(options) > 0 {
		built := make([]huh.Option[string], 0, len(options))
		for _, option := range options {
			item := huh.NewOption(option.Key, option.Value)
			if option.Selected {
				item = item.Selected(true)
			}
			built = append(built, item)
		}
		field.Options(built...)
	}
	_, err := p.run(field)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(selected), nil
}

type lineRunReader struct {
	state        *promptState
	pending      []byte
	eofAfterLine bool
	hitEOF       bool
}

func (r *lineRunReader) Read(p []byte) (int, error) {
	if len(r.pending) == 0 {
		line, err := r.state.reader.ReadBytes('\n')
		switch {
		case err == nil:
			r.pending = line
		case errors.Is(err, io.EOF):
			r.hitEOF = true
			if len(line) == 0 {
				return 0, io.EOF
			}
			r.pending = line
			r.eofAfterLine = true
		default:
			return 0, err
		}
	}
	n := copy(p, r.pending)
	r.pending = r.pending[n:]
	if len(r.pending) == 0 && r.eofAfterLine {
		return n, io.EOF
	}
	return n, nil
}

func (p PromptIO) run(field huh.Field) (bool, error) {
	input := p.In
	if input == nil {
		input = os.Stdin
	}
	output := p.Out
	if output == nil {
		output = os.Stderr
	}
	form := huh.NewForm(huh.NewGroup(field)).WithOutput(output).WithAccessible(p.Accessible).WithShowHelp(false)
	hitEOF := false
	if p.Accessible {
		reader := &lineRunReader{state: p.promptState()}
		form.WithInput(reader)
		if err := form.Run(); err != nil {
			if errors.Is(err, huh.ErrUserAborted) {
				return reader.hitEOF, io.EOF
			}
			return reader.hitEOF, err
		}
		return reader.hitEOF, nil
	}
	form.WithInput(input)
	if err := form.Run(); err != nil {
		if errors.Is(err, huh.ErrUserAborted) {
			return hitEOF, io.EOF
		}
		return hitEOF, err
	}
	return hitEOF, nil
}

func (p PromptIO) Close() error {
	state := p.promptState()
	if state.lineEditor == nil {
		return nil
	}
	return state.lineEditor.Close()
}

func (p PromptIO) promptState() *promptState {
	if p.state != nil {
		return p.state
	}
	return newPromptState(p.In)
}

func (p PromptIO) lineEditor() lineEditor {
	if p.Accessible {
		return nil
	}
	state := p.promptState()
	state.lineEditorOnce.Do(func() {
		state.lineEditor, _ = newLineEditorFunc(p.In, p.Out)
	})
	return state.lineEditor
}

func newPromptState(input io.Reader) *promptState {
	if reader, ok := input.(*bufio.Reader); ok {
		return &promptState{reader: reader}
	}
	return &promptState{reader: bufio.NewReader(input)}
}

func newInteractiveLineEditor(input io.Reader, output io.Writer) (lineEditor, error) {
	stdin, ok := input.(*os.File)
	if !ok || !InteractiveIO(input, output) {
		return nil, nil
	}
	if output == nil {
		output = os.Stderr
	}
	return readline.NewEx(&readline.Config{
		DisableAutoSaveHistory: false,
		ForceUseInteractive:    true,
		HistoryLimit:           500,
		HistorySearchFold:      true,
		Prompt:                 "",
		Stderr:                 output,
		Stdin:                  nopReadCloser{Reader: stdin},
		Stdout:                 output,
	})
}

func isTTY(stream any) bool {
	file, ok := stream.(*os.File)
	if !ok {
		return false
	}
	info, err := file.Stat()
	if err != nil {
		return false
	}
	return (info.Mode() & os.ModeCharDevice) != 0
}
