package aws

import (
	"strings"
	"testing"
	"time"
)

func TestSignV4Headers(t *testing.T) {
	headers, err := SignV4Headers(
		Credentials{AccessKey: "AKIDEXAMPLE", SecretKey: "wJalrXUtnFEMI/I/K7MDENG+bPxRfiCYEXAMPLEKEY", SessionToken: "TOKEN"},
		"ap-southeast-2",
		"bedrock-mantle",
		"POST",
		"https://bedrock-mantle.ap-southeast-2.api.aws/v1/chat/completions",
		[]byte(`{"model":"zai.glm-4.6"}`),
		map[string]string{"Content-Type": "application/json"},
		time.Date(2026, 3, 27, 6, 20, 9, 0, time.UTC),
	)
	if err != nil {
		t.Fatal(err)
	}
	if headers["Content-Type"] != "application/json" {
		t.Fatalf("missing content type: %#v", headers)
	}
	if headers["Host"] != "bedrock-mantle.ap-southeast-2.api.aws" {
		t.Fatalf("unexpected host: %#v", headers)
	}
	if headers["X-Amz-Security-Token"] != "TOKEN" {
		t.Fatalf("unexpected token: %#v", headers)
	}
	if !strings.Contains(headers["Authorization"], "Credential=AKIDEXAMPLE/") {
		t.Fatalf("missing credential in auth: %s", headers["Authorization"])
	}
	if !strings.Contains(headers["Authorization"], "/ap-southeast-2/bedrock-mantle/aws4_request") {
		t.Fatalf("missing scope in auth: %s", headers["Authorization"])
	}
}
