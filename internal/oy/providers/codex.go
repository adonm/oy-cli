package providers

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"sort"
	"strings"
	"time"
)

func loadCodexModelList() []string {
	data := loadJSONObject(CodexModelsCachePath)
	models := extractModelIDs(data["models"], "id", "name", "slug", "model", "model_id")
	if len(models) == 0 {
		auth := LoadCodexAuth()
		models = extractModelIDs(auth["models"], "id", "slug", "name")
		if len(models) == 0 {
			for _, key := range []string{"model", "default_model"} {
				if value, ok := auth[key].(string); ok && strings.TrimSpace(value) != "" {
					models = append(models, value)
				}
			}
		}
	}
	if len(models) == 0 {
		return nil
	}
	seen := map[string]struct{}{}
	out := make([]string, 0, len(models))
	for _, item := range models {
		if _, ok := seen[item]; ok {
			continue
		}
		seen[item] = struct{}{}
		out = append(out, item)
	}
	sort.Strings(out)
	return out
}

func codexOAuthClientID() string {
	if value := strings.TrimSpace(os.Getenv("CODEX_OAUTH_CLIENT_ID")); value != "" {
		return value
	}
	return codexOAuthClientIDDefault
}

func postFormJSON(rawURL string, data map[string]string, errorPrefix string) (map[string]any, error) {
	form := url.Values{}
	for key, value := range data {
		form.Set(key, value)
	}
	body := []byte(form.Encode())
	response, err := ToolSession(ShortHTTPTimeout, false).Request(http.MethodPost, rawURL, map[string]string{"Content-Type": "application/x-www-form-urlencoded"}, body)
	if err != nil {
		return nil, fmt.Errorf("%s: %v", errorPrefix, err)
	}
	if err := ResponseRaiseForStatus(response); err != nil {
		return nil, fmt.Errorf("%s: %v", errorPrefix, err)
	}
	return responseJSONObject(response, errorPrefix+": invalid JSON response")
}

func codexTokens(auth map[string]any) (map[string]string, error) {
	tokens, _ := auth["tokens"].(map[string]any)
	if tokens == nil {
		return nil, fmt.Errorf("Codex CLI auth file does not contain session tokens")
	}
	result := map[string]string{}
	for _, key := range []string{"access_token", "refresh_token", "id_token", "account_id"} {
		if value, ok := tokens[key].(string); ok && strings.TrimSpace(value) != "" {
			result[key] = value
		}
	}
	return result, nil
}

func refreshCodexChatGPTSession(refreshToken string) (map[string]any, error) {
	data, err := postFormJSON(CodexOAuthTokenURL, map[string]string{
		"grant_type":    "refresh_token",
		"refresh_token": refreshToken,
		"client_id":     codexOAuthClientID(),
	}, "Codex token refresh failed")
	if err != nil {
		return nil, err
	}
	accessToken, _ := data["access_token"].(string)
	if strings.TrimSpace(accessToken) == "" {
		return nil, fmt.Errorf("Codex token refresh did not return an access_token")
	}
	auth := LoadCodexAuth()
	tokens, _ := auth["tokens"].(map[string]any)
	if tokens == nil {
		tokens = map[string]any{}
	}
	tokens["access_token"] = accessToken
	for _, key := range []string{"refresh_token", "id_token"} {
		if value, ok := data[key].(string); ok && strings.TrimSpace(value) != "" {
			tokens[key] = value
		}
	}
	auth["tokens"] = tokens
	auth["last_refresh"] = time.Now().UTC().Format(time.RFC3339)
	SaveJSON(CodexAuthPath, auth)
	return auth, nil
}

func GetCodexChatGPTSession(forceRefresh bool) (map[string]string, error) {
	auth, err := LoadCodexSession()
	if err != nil {
		return nil, err
	}
	tokens, err := codexTokens(auth)
	if err != nil {
		return nil, err
	}
	accessToken := tokens["access_token"]
	refreshToken := tokens["refresh_token"]
	accountID := tokens["account_id"]
	if refreshToken == "" || accountID == "" {
		return nil, fmt.Errorf("Codex CLI auth file does not contain a usable ChatGPT session.")
	}
	expiry := DecodeJWTExpiryEpoch(accessToken)
	if forceRefresh || accessToken == "" || (expiry != nil && *expiry <= float64(time.Now().UTC().Unix()+60)) {
		refreshed, err := refreshCodexChatGPTSession(refreshToken)
		if err != nil {
			return nil, err
		}
		tokens, err = codexTokens(refreshed)
		if err != nil {
			return nil, err
		}
		accessToken = tokens["access_token"]
		accountID = tokens["account_id"]
		if value := tokens["refresh_token"]; value != "" {
			refreshToken = value
		}
	}
	if accessToken == "" || accountID == "" {
		return nil, fmt.Errorf("Codex ChatGPT session is missing access token or account ID")
	}
	return map[string]string{"access_token": accessToken, "refresh_token": refreshToken, "account_id": accountID}, nil
}

func httpErrorMessage(prefix string, response ResponseAdapter) string {
	payload, err := ResponseJSON(response)
	if err != nil {
		body := strings.TrimSpace(response.Text)
		if len(body) > 200 {
			body = body[:200]
		}
		if body == "" {
			body = response.ReasonPhrase
		}
		return fmt.Sprintf("%s error %d: %s", prefix, response.StatusCode, body)
	}
	detail := any(payload)
	if data, ok := payload.(map[string]any); ok {
		if value := data["error"]; value != nil {
			detail = value
		} else if value := data["detail"]; value != nil {
			detail = value
		}
	}
	switch value := detail.(type) {
	case map[string]any:
		if message, ok := value["message"].(string); ok && message != "" {
			return fmt.Sprintf("%s error %d: %s", prefix, response.StatusCode, message)
		}
		if code, ok := value["code"].(string); ok && code != "" {
			return fmt.Sprintf("%s error %d: %s", prefix, response.StatusCode, code)
		}
		encoded, _ := json.Marshal(value)
		return fmt.Sprintf("%s error %d: %s", prefix, response.StatusCode, string(encoded))
	case string:
		return fmt.Sprintf("%s error %d: %s", prefix, response.StatusCode, value)
	default:
		encoded, _ := json.Marshal(value)
		return fmt.Sprintf("%s error %d: %s", prefix, response.StatusCode, string(encoded))
	}
}

func hasMeaningfulAssistantOutput(message ChatMessage) bool {
	return len(message.ToolCalls) > 0 || !isBlankChatValue(message.Content)
}

func CodexChatGPTClient() CompletionClient {
	client := NewOpenAIHTTPClient("", CodexChatGPTBaseURL, nil, DefaultHTTPTimeout)
	return &funcClient{
		chatCompletion: func(model string, messages []ChatMessage, tools []map[string]any, toolChoice string) (ChatMessage, error) {
			payload := responsesPayload(model, messages, tools, toolChoice)
			var lastDecodeErr error
			for attempt := 0; attempt < 2; attempt++ {
				session, err := GetCodexChatGPTSession(attempt > 0)
				if err != nil {
					return ChatMessage{}, err
				}
				response, err := client.request("POST", "/responses", mustJSONBody(payload), map[string]string{
					"Authorization":      "Bearer " + session["access_token"],
					"ChatGPT-Account-Id": session["account_id"],
				})
				if err != nil {
					if _, ok := err.(*AuthenticationError); ok && attempt == 0 {
						continue
					}
					statusErr := &APIStatusError{}
					if AsAPIStatusError(err, &statusErr) {
						return ChatMessage{}, fmt.Errorf("%s", httpErrorMessage("Codex ChatGPT", statusErr.Response))
					}
					return ChatMessage{}, fmt.Errorf("Codex ChatGPT request failed: %v", err)
				}
				data, err := responseJSONObject(response, "Codex ChatGPT: invalid JSON response")
				if err != nil {
					lastDecodeErr = err
					continue
				}
				message, err := decodeResponsesOutput(data)
				if err != nil {
					lastDecodeErr = err
					continue
				}
				if hasMeaningfulAssistantOutput(message) {
					return message, nil
				}
				lastDecodeErr = fmt.Errorf("malformed model output: empty assistant message with no tool calls")
			}
			if lastDecodeErr != nil {
				return ChatMessage{}, lastDecodeErr
			}
			return ChatMessage{}, fmt.Errorf("Codex ChatGPT authentication failed after token refresh")
		},
		listModels: func() ([]string, error) {
			items := loadCodexModelList()
			if len(items) == 0 {
				return []string{CodexDefaultModel}, nil
			}
			return items, nil
		},
	}
}
