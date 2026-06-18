package snapshot

import (
	"context"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
)

func TestBuildRuntimeSnapshotExportsCodexOAuthAuth(t *testing.T) {
	now := time.Date(2026, 6, 18, 10, 0, 0, 0, time.UTC)
	cfg := &config.Config{
		DataPlane: config.DataPlaneConfig{
			ResponsesBaseURL: "http://127.0.0.1:4100",
		},
		Routing: config.RoutingConfig{
			Strategy:           "fill-first",
			SessionAffinity:    true,
			SessionAffinityTTL: "45m",
		},
		CodexHeaderDefaults: config.CodexHeaderDefaults{
			UserAgent: "codex-tui/custom",
		},
		OAuthModelAlias: map[string][]config.OAuthModelAlias{
			"codex": {
				{Name: "gpt-5.3-codex-spark", Alias: "codex-latest"},
			},
		},
	}
	manager := coreauth.NewManager(nil, nil, nil)
	_, err := manager.Register(context.Background(), &coreauth.Auth{
		ID:       "auth-codex-1",
		Provider: "codex",
		Status:   coreauth.StatusActive,
		Attributes: map[string]string{
			"auth_kind": "oauth",
			"priority":  "120",
			"plan_type": "plus",
			"base_url":  "https://chatgpt.com/backend-api/codex",
		},
		Metadata: map[string]any{
			"access_token": "token-1",
			"account_id":   "acct-1",
		},
	})
	if err != nil {
		t.Fatalf("register auth: %v", err)
	}

	snapshot := BuildRuntimeSnapshot(cfg, manager, now)
	if snapshot.Routes.Responses != true {
		t.Fatalf("routes.responses = %v, want true", snapshot.Routes.Responses)
	}
	if snapshot.Listeners.PublicHTTP != "http://127.0.0.1:4100" {
		t.Fatalf("listeners.public_http = %q, want http://127.0.0.1:4100", snapshot.Listeners.PublicHTTP)
	}
	if snapshot.Routing.Strategy != "fill-first" {
		t.Fatalf("routing.strategy = %q, want fill-first", snapshot.Routing.Strategy)
	}
	if snapshot.Routing.SessionTTLSeconds != 2700 {
		t.Fatalf("routing.session_ttl_seconds = %d, want 2700", snapshot.Routing.SessionTTLSeconds)
	}
	if snapshot.Providers["codex"].Enabled != true {
		t.Fatalf("providers.codex.enabled = %v, want true", snapshot.Providers["codex"].Enabled)
	}
	if got := snapshot.ModelAliases["codex"]["codex-latest"]; got != "gpt-5.3-codex-spark" {
		t.Fatalf("model alias = %q, want gpt-5.3-codex-spark", got)
	}
	if len(snapshot.AuthPool) != 1 {
		t.Fatalf("auth_pool len = %d, want 1", len(snapshot.AuthPool))
	}
	auth := snapshot.AuthPool[0]
	if auth.ID != "auth-codex-1" {
		t.Fatalf("auth.id = %q, want auth-codex-1", auth.ID)
	}
	if auth.Priority != 120 {
		t.Fatalf("auth.priority = %d, want 120", auth.Priority)
	}
	if auth.Execution.Codex == nil {
		t.Fatal("auth.execution.codex = nil")
	}
	if auth.Execution.Codex.AccessToken != "token-1" {
		t.Fatalf("access_token = %q, want token-1", auth.Execution.Codex.AccessToken)
	}
	if auth.Execution.Codex.UserAgent != "codex-tui/custom" {
		t.Fatalf("user_agent = %q, want codex-tui/custom", auth.Execution.Codex.UserAgent)
	}
	if auth.Execution.Codex.OpenAIBeta != "responses=v1" {
		t.Fatalf("openai_beta = %q, want responses=v1", auth.Execution.Codex.OpenAIBeta)
	}
	if len(auth.SupportsModels) == 0 {
		t.Fatal("supports_models should not be empty")
	}
}

func TestBuildRuntimeSnapshotSkipsDisabledAndMissingTokenAuths(t *testing.T) {
	manager := coreauth.NewManager(nil, nil, nil)
	_, _ = manager.Register(context.Background(), &coreauth.Auth{
		ID:       "disabled",
		Provider: "codex",
		Status:   coreauth.StatusDisabled,
		Disabled: true,
		Attributes: map[string]string{
			"auth_kind": "oauth",
		},
		Metadata: map[string]any{
			"access_token": "token-disabled",
		},
	})
	_, _ = manager.Register(context.Background(), &coreauth.Auth{
		ID:       "missing-token",
		Provider: "codex",
		Status:   coreauth.StatusActive,
		Attributes: map[string]string{
			"auth_kind": "oauth",
		},
		Metadata: map[string]any{},
	})

	snapshot := BuildRuntimeSnapshot(&config.Config{}, manager, time.Date(2026, 6, 18, 10, 0, 0, 0, time.UTC))
	if len(snapshot.AuthPool) != 0 {
		t.Fatalf("auth_pool len = %d, want 0", len(snapshot.AuthPool))
	}
	if snapshot.Providers["codex"].Enabled {
		t.Fatalf("providers.codex.enabled = true, want false")
	}
}

func TestBuildRuntimeSnapshotExportsCooldownAndStableVersion(t *testing.T) {
	now := time.Date(2026, 6, 18, 10, 0, 0, 0, time.UTC)
	manager := coreauth.NewManager(nil, nil, nil)
	auth := &coreauth.Auth{
		ID:       "auth-cooldown",
		Provider: "codex",
		Status:   coreauth.StatusActive,
		Attributes: map[string]string{
			"auth_kind": "oauth",
			"plan_type": "free",
		},
		Metadata: map[string]any{
			"access_token": "token-1",
		},
		NextRetryAfter: now.Add(10 * time.Minute),
	}
	_, _ = manager.Register(context.Background(), auth)

	first := BuildRuntimeSnapshot(&config.Config{}, manager, now)
	second := BuildRuntimeSnapshot(&config.Config{}, manager, now)
	if first.Version != second.Version {
		t.Fatalf("version mismatch: %q != %q", first.Version, second.Version)
	}
	if len(first.AuthPool) != 1 || first.AuthPool[0].CooldownUntil == nil {
		t.Fatalf("cooldown_until not exported: %#v", first.AuthPool)
	}

	updated := auth.Clone()
	updated.Metadata["access_token"] = "token-2"
	_, _ = manager.Update(context.Background(), updated)
	third := BuildRuntimeSnapshot(&config.Config{}, manager, now)
	if third.Version == first.Version {
		t.Fatalf("version should change when auth content changes: %q", third.Version)
	}
}
