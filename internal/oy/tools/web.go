package tools

import (
	"fmt"
	"net"
	"net/url"
	"strings"

	"github.com/wagov-dtt/oy-cli/internal/oy/providers"
	"github.com/wagov-dtt/oy-cli/internal/oy/runtime"
)

func ValidateURLSafe(raw string) error {
	parsed, err := url.Parse(raw)
	if err != nil {
		return err
	}
	if parsed.Scheme != "http" && parsed.Scheme != "https" {
		return fmt.Errorf("only http/https URLs are allowed, got: %q", parsed.Scheme)
	}
	hostname := parsed.Hostname()
	if hostname == "" {
		return fmt.Errorf("no hostname in URL: %q", raw)
	}
	for _, blocked := range []string{"localhost", "localhost.localdomain", "ip6-localhost", "ip6-loopback"} {
		if strings.EqualFold(hostname, blocked) {
			return fmt.Errorf("local addresses are not allowed: %q", hostname)
		}
	}
	ips, err := net.LookupIP(hostname)
	if err != nil {
		return fmt.Errorf("cannot resolve hostname %q: %v", hostname, err)
	}
	for _, ip := range ips {
		if ip.IsLoopback() || ip.IsPrivate() || ip.IsLinkLocalMulticast() || ip.IsLinkLocalUnicast() || ip.IsUnspecified() {
			return fmt.Errorf("URL resolves to non-public address (%s); private/reserved/loopback/link-local addresses are blocked", ip.String())
		}
	}
	return nil
}

func ToolWebfetch(state State, rawURL, method string, headers map[string]string, followRedirects bool, timeoutSeconds int) (map[string]any, error) {
	method = strings.ToUpper(strings.TrimSpace(method))
	if method == "" {
		method = "GET"
	}
	if _, ok := map[string]struct{}{"GET": {}, "HEAD": {}, "OPTIONS": {}}[method]; !ok {
		return nil, fmt.Errorf("Only GET, HEAD, OPTIONS methods are allowed, got: %q", method)
	}
	if err := ValidateURLSafe(rawURL); err != nil {
		return nil, err
	}
	cleanHeaders, err := sanitizeRequestHeaders(headers)
	if err != nil {
		return nil, err
	}
	response, err := ToolSessionFactory(timeDurationSeconds(timeoutSeconds), followRedirects).Request(method, rawURL, cleanHeaders, nil)
	if err != nil {
		return map[string]any{
			"method":     method,
			"url":        rawURL,
			"ok":         false,
			"error_type": errorTypeName(err),
			"message":    err.Error(),
		}, nil
	}
	if !textResponse(response) {
		return WebfetchPayload(response, method, nil, false, "binary"), nil
	}
	text := response.Text
	format := "text"
	if htmlResponse(response, text) {
		text = htmlToMarkdown(text)
		format = "markdown"
	}
	summarized, truncated := summarizeText(text, runtime.DefaultBudgets().ToolOutputTokens*8)
	return WebfetchPayload(response, method, &summarized, truncated, format), nil
}

func WebfetchPayload(response providers.ResponseAdapter, method string, text *string, truncated bool, format string) map[string]any {
	payload := map[string]any{
		"method":        method,
		"url":           response.URL,
		"status_code":   response.StatusCode,
		"reason_phrase": response.ReasonPhrase,
		"http_version":  response.HTTPVersion,
		"headers":       webfetchHeaders(response.Headers),
	}
	if text == nil {
		payload["binary"] = true
		payload["content_bytes"] = len(response.Content)
		return payload
	}
	payload["text"] = *text
	payload["format"] = format
	payload["truncated"] = truncated
	return payload
}
