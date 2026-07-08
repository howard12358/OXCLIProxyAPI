package openai

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/gin-gonic/gin"
	internalconfig "github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/registry"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/runtime/executor"
	_ "github.com/router-for-me/CLIProxyAPI/v7/internal/translator/codex/openai/responses"
	"github.com/router-for-me/CLIProxyAPI/v7/sdk/api/handlers"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
	sdkconfig "github.com/router-for-me/CLIProxyAPI/v7/sdk/config"
)

type responsesGoldenFixture struct {
	Request          map[string]any `json:"request"`
	ExpectedResponse map[string]any `json:"expected_response"`
}

func TestOpenAIResponsesNativeCodexMatchesSharedGoldenFixture(t *testing.T) {
	gin.SetMode(gin.TestMode)

	fixture := loadResponsesGoldenFixture(t, "non_stream_aggregates_codex_stream.json")
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/responses" {
			t.Errorf("upstream path = %q, want /responses", r.URL.Path)
		}
		if got := r.Header.Get("Authorization"); got != "Bearer codex-token-a" {
			t.Errorf("authorization = %q, want Bearer codex-token-a", got)
		}
		if got := r.Header.Get("Chatgpt-Account-Id"); got != "acct_a" {
			t.Errorf("chatgpt-account-id = %q, want acct_a", got)
		}

		body, err := io.ReadAll(r.Body)
		if err != nil {
			t.Fatalf("read upstream request body: %v", err)
		}
		var payload map[string]any
		if err := json.Unmarshal(body, &payload); err != nil {
			t.Fatalf("parse upstream request body: %v", err)
		}
		payloadJSON, err := json.Marshal(payload)
		if err != nil {
			t.Fatalf("marshal upstream payload: %v", err)
		}

		w.Header().Set("Content-Type", "text/event-stream")
		_, _ = fmt.Fprintf(w, concatResponsesCompletedSSE,
			"Bearer codex-token-a",
			"acct_a",
			mustMarshalJSON(t, payload["model"]),
			payloadJSON,
		)
	}))
	defer upstream.Close()

	manager := coreauth.NewManager(nil, nil, nil)
	manager.RegisterExecutor(executor.NewCodexExecutor(&internalconfig.Config{
		SDKConfig: internalconfig.SDKConfig{
			DisableImageGeneration: internalconfig.DisableImageGenerationAll,
		},
	}))
	manager.SetOAuthModelAlias(map[string][]internalconfig.OAuthModelAlias{
		"codex": {{
			Name:  "gpt-5-codex",
			Alias: "codex-latest",
			Fork:  true,
		}},
	})

	auth := &coreauth.Auth{
		ID:       "responses-go-native-codex",
		Provider: "codex",
		Status:   coreauth.StatusActive,
		Attributes: map[string]string{
			"base_url": upstream.URL,
		},
		Metadata: map[string]any{
			"access_token": "codex-token-a",
			"account_id":   "acct_a",
			"email":        "codex@example.test",
		},
	}
	if _, err := manager.Register(context.Background(), auth); err != nil {
		t.Fatalf("register auth: %v", err)
	}
	registry.GetGlobalRegistry().RegisterClient(auth.ID, auth.Provider, []*registry.ModelInfo{{ID: "codex-latest"}, {ID: "gpt-5-codex"}})
	t.Cleanup(func() {
		registry.GetGlobalRegistry().UnregisterClient(auth.ID)
	})

	base := handlers.NewBaseAPIHandlers(&sdkconfig.SDKConfig{}, manager)
	h := NewOpenAIResponsesAPIHandler(base)
	router := gin.New()
	router.POST("/v1/responses", h.Responses)

	requestBody := mustMarshalJSON(t, fixture.Request)
	req := httptest.NewRequest(http.MethodPost, "/v1/responses", strings.NewReader(string(requestBody)))
	req.Header.Set("Content-Type", "application/json")
	resp := httptest.NewRecorder()
	router.ServeHTTP(resp, req)

	if resp.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d; body=%s", resp.Code, http.StatusOK, resp.Body.String())
	}
	var actual map[string]any
	if err := json.Unmarshal(resp.Body.Bytes(), &actual); err != nil {
		t.Fatalf("parse response body: %v; body=%s", err, resp.Body.String())
	}
	assertJSONEqual(t, actual, fixture.ExpectedResponse)
}

const concatResponsesCompletedSSE = "event: response.created\n" +
	"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-stream-1\",\"status\":\"in_progress\"}}\n\n" +
	"event: response.completed\n" +
	"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-stream-1\",\"object\":\"response\",\"status\":\"completed\",\"provider\":\"openai\",\"auth\":%q,\"account_id\":%q,\"model\":%s,\"received_payload\":%s,\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"stream ok\"}]}]}}\n\n"

func loadResponsesGoldenFixture(t *testing.T, name string) responsesGoldenFixture {
	t.Helper()
	root := repoRoot(t)
	raw, err := os.ReadFile(filepath.Join(root, "testdata", "contract", "responses", name))
	if err != nil {
		t.Fatalf("read responses golden fixture: %v", err)
	}
	var fixture responsesGoldenFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatalf("parse responses golden fixture: %v", err)
	}
	return fixture
}

func repoRoot(t *testing.T) string {
	t.Helper()
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	dir := filepath.Dir(file)
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.mod")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatal("repo root not found")
		}
		dir = parent
	}
}

func mustMarshalJSON(t *testing.T, value any) []byte {
	t.Helper()
	out, err := json.Marshal(value)
	if err != nil {
		t.Fatalf("marshal JSON: %v", err)
	}
	return out
}

func assertJSONEqual(t *testing.T, actual, expected any) {
	t.Helper()
	actualJSON, err := json.Marshal(actual)
	if err != nil {
		t.Fatalf("marshal actual JSON: %v", err)
	}
	expectedJSON, err := json.Marshal(expected)
	if err != nil {
		t.Fatalf("marshal expected JSON: %v", err)
	}
	if string(actualJSON) != string(expectedJSON) {
		t.Fatalf("response JSON mismatch\nactual:   %s\nexpected: %s", actualJSON, expectedJSON)
	}
}
