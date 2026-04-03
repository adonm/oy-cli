package aws

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/url"
	"sort"
	"strings"
	"time"
)

type Credentials struct {
	AccessKey    string
	SecretKey    string
	SessionToken string
}

func SignV4Headers(credentials Credentials, region, service, method, rawURL string, body []byte, headers map[string]string, now time.Time) (map[string]string, error) {
	if now.IsZero() {
		now = time.Now().UTC()
	}
	parsed, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	canonicalHeaders := map[string]string{
		"host":       parsed.Host,
		"x-amz-date": now.Format("20060102T150405Z"),
	}
	for key, value := range headers {
		canonicalHeaders[strings.ToLower(key)] = strings.TrimSpace(value)
	}
	if credentials.SessionToken != "" {
		canonicalHeaders["x-amz-security-token"] = credentials.SessionToken
	}
	signedHeaderNames := make([]string, 0, len(canonicalHeaders))
	for key := range canonicalHeaders {
		signedHeaderNames = append(signedHeaderNames, key)
	}
	sort.Strings(signedHeaderNames)
	datestamp := now.Format("20060102")
	signedHeaders := strings.Join(signedHeaderNames, ";")
	canonicalRequest := strings.Join([]string{
		strings.ToUpper(method),
		normalizePath(parsed.EscapedPath()),
		canonicalQueryString(parsed.RawQuery),
		canonicalHeaderBlock(canonicalHeaders, signedHeaderNames),
		signedHeaders,
		hexDigest(body),
	}, "\n")
	scope := fmt.Sprintf("%s/%s/%s/aws4_request", datestamp, region, service)
	stringToSign := strings.Join([]string{
		"AWS4-HMAC-SHA256",
		now.Format("20060102T150405Z"),
		scope,
		hexDigest([]byte(canonicalRequest)),
	}, "\n")
	signature := hex.EncodeToString(signatureKey(credentials.SecretKey, datestamp, region, service, stringToSign))
	out := map[string]string{}
	for key, value := range headers {
		out[key] = value
	}
	out["Host"] = parsed.Host
	out["X-Amz-Date"] = now.Format("20060102T150405Z")
	if credentials.SessionToken != "" {
		out["X-Amz-Security-Token"] = credentials.SessionToken
	}
	out["Authorization"] = fmt.Sprintf(
		"AWS4-HMAC-SHA256 Credential=%s/%s, SignedHeaders=%s, Signature=%s",
		credentials.AccessKey,
		scope,
		signedHeaders,
		signature,
	)
	return out, nil
}

func normalizePath(path string) string {
	if path == "" {
		return "/"
	}
	if !strings.HasPrefix(path, "/") {
		return "/" + path
	}
	return path
}

func canonicalQueryString(raw string) string {
	if raw == "" {
		return ""
	}
	parts := strings.Split(raw, "&")
	sort.Strings(parts)
	return strings.Join(parts, "&")
}

func canonicalHeaderBlock(headers map[string]string, keys []string) string {
	parts := make([]string, 0, len(keys))
	for _, key := range keys {
		parts = append(parts, key+":"+headers[key])
	}
	return strings.Join(parts, "\n") + "\n"
}

func hexDigest(data []byte) string {
	sum := sha256.Sum256(data)
	return hex.EncodeToString(sum[:])
}

func hmacSHA256(key []byte, value string) []byte {
	mac := hmac.New(sha256.New, key)
	_, _ = mac.Write([]byte(value))
	return mac.Sum(nil)
}

func signatureKey(secretKey, datestamp, region, service, stringToSign string) []byte {
	kDate := hmacSHA256([]byte("AWS4"+secretKey), datestamp)
	kRegion := hmacSHA256(kDate, region)
	kService := hmacSHA256(kRegion, service)
	kSigning := hmacSHA256(kService, "aws4_request")
	return hmacSHA256(kSigning, stringToSign)
}
