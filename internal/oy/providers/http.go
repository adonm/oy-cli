package providers

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const (
	DefaultHTTPTimeout            = 300 * time.Second
	ShortHTTPTimeout              = 30 * time.Second
	DefaultWebfetchTimeoutSeconds = 60 * time.Second
)

type HTTPError struct {
	Message  string
	Response *ResponseAdapter
}

func (e *HTTPError) Error() string { return e.Message }

type TransportError struct{ Message string }

func (e *TransportError) Error() string { return e.Message }

type TimeoutException struct{ Message string }

func (e *TimeoutException) Error() string { return e.Message }

type APIConnectionError struct{ Message string }

func (e *APIConnectionError) Error() string { return e.Message }

type APITimeoutError struct{ Message string }

func (e *APITimeoutError) Error() string { return e.Message }

type APIStatusError struct {
	Message  string
	Response ResponseAdapter
	Body     any
}

func (e *APIStatusError) Error() string { return e.Message }

type AuthenticationError struct{ APIStatusError }
type PermissionDeniedError struct{ APIStatusError }
type RateLimitError struct{ APIStatusError }
type BadRequestError struct{ APIStatusError }

func AdaptResponse(statusCode int, headers http.Header, text string, content []byte, url, reasonPhrase, httpVersion string) ResponseAdapter {
	return ResponseAdapter{
		StatusCode:   statusCode,
		Headers:      normalizeHeaders(headers),
		Text:         text,
		Content:      content,
		URL:          url,
		ReasonPhrase: reasonPhrase,
		HTTPVersion:  httpVersion,
	}
}

func ResponseIsSuccess(response ResponseAdapter) bool {
	return response.StatusCode >= 200 && response.StatusCode < 300
}

func ResponseJSON(response ResponseAdapter) (any, error) {
	var data any
	err := json.Unmarshal([]byte(response.Text), &data)
	return data, err
}

func ResponseRaiseForStatus(response ResponseAdapter) error {
	if response.StatusCode < 400 {
		return nil
	}
	message := ResponseErrorMessage(response)
	if strings.TrimSpace(message) == "" {
		message = response.ReasonPhrase
	}
	if strings.TrimSpace(message) == "" {
		message = fmt.Sprintf("HTTP %d", response.StatusCode)
	}
	base := APIStatusError{Message: message, Response: response}
	switch response.StatusCode {
	case 400:
		return &BadRequestError{APIStatusError: base}
	case 401:
		return &AuthenticationError{APIStatusError: base}
	case 403:
		return &PermissionDeniedError{APIStatusError: base}
	case 429:
		return &RateLimitError{APIStatusError: base}
	default:
		return &base
	}
}

func ResponseErrorMessage(response ResponseAdapter) string {
	payload, err := ResponseJSON(response)
	if err == nil {
		if data, ok := payload.(map[string]any); ok {
			if errorItem, ok := data["error"].(map[string]any); ok {
				if message, ok := errorItem["message"].(string); ok {
					return message
				}
			}
			if message, ok := data["message"].(string); ok && message != "" {
				if code, ok := data["code"].(string); ok && code != "" {
					return code + ": " + message
				}
				return message
			}
		}
	}
	return strings.TrimSpace(response.Text)
}

type HTTPClient struct {
	Timeout         time.Duration
	FollowRedirects bool
	client          *http.Client
}

func NewHTTPClient(timeout time.Duration, followRedirects bool) *HTTPClient {
	client := &http.Client{Timeout: timeout}
	if !followRedirects {
		client.CheckRedirect = func(req *http.Request, via []*http.Request) error {
			return http.ErrUseLastResponse
		}
	}
	return &HTTPClient{Timeout: timeout, FollowRedirects: followRedirects, client: client}
}

func (c *HTTPClient) Request(method, url string, headers map[string]string, body []byte) (ResponseAdapter, error) {
	request, err := http.NewRequest(strings.ToUpper(method), url, bytes.NewReader(body))
	if err != nil {
		return ResponseAdapter{}, err
	}
	for key, value := range headers {
		request.Header.Set(key, value)
	}
	response, err := c.client.Do(request)
	if err != nil {
		if strings.Contains(strings.ToLower(err.Error()), "timeout") {
			return ResponseAdapter{}, &APITimeoutError{Message: err.Error()}
		}
		return ResponseAdapter{}, &APIConnectionError{Message: err.Error()}
	}
	defer response.Body.Close()
	content, err := io.ReadAll(response.Body)
	if err != nil {
		return ResponseAdapter{}, &TransportError{Message: err.Error()}
	}
	adapted := AdaptResponse(
		response.StatusCode,
		response.Header,
		string(content),
		content,
		response.Request.URL.String(),
		response.Status,
		response.Proto,
	)
	return adapted, nil
}

func LLMSession(timeout time.Duration, followRedirects bool) *HTTPClient {
	if timeout <= 0 {
		timeout = DefaultHTTPTimeout
	}
	return NewHTTPClient(timeout, followRedirects)
}

func ToolSession(timeout time.Duration, followRedirects bool) *HTTPClient {
	if timeout <= 0 {
		timeout = DefaultWebfetchTimeoutSeconds
	}
	return NewHTTPClient(timeout, followRedirects)
}

func normalizeHeaders(headers http.Header) map[string]string {
	if headers == nil {
		return map[string]string{}
	}
	out := make(map[string]string, len(headers))
	for key, values := range headers {
		out[strings.ToLower(key)] = strings.Join(values, ", ")
	}
	return out
}
