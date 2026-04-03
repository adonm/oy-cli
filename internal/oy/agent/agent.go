package agent

import "github.com/wagov-dtt/oy-cli/internal/oy/providers"

type Transcript struct {
	Messages         []providers.ChatMessage `json:"messages"`
	MaxContextTokens int                     `json:"max_context_tokens"`
	MaxMessageTokens int                     `json:"max_message_tokens"`
}

func TranscriptWithSystemPrompt(systemPrompt string) Transcript {
	return Transcript{Messages: []providers.ChatMessage{providers.SystemMessage(systemPrompt)}}
}
