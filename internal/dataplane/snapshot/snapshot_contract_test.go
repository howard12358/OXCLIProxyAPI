package snapshot

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
)

func TestBuildRuntimeSnapshotGolden(t *testing.T) {
	now := time.Date(2026, 6, 18, 10, 0, 0, 0, time.UTC)
	cfg := &config.Config{
		SDKConfig: config.SDKConfig{
			ProxyURL: "socks5h://127.0.0.1:7897",
		},
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
		RemoteManagement: config.RemoteManagement{
			SecretKey: "management-key",
		},
		UsageStatisticsEnabled: true,
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
			"email":        "codex-user@example.com",
		},
	})
	if err != nil {
		t.Fatalf("register auth: %v", err)
	}

	snapshot := BuildRuntimeSnapshot(cfg, manager, now)
	got, err := normalizeSnapshotJSON(snapshot)
	if err != nil {
		t.Fatalf("normalize snapshot: %v", err)
	}

	goldenPath := filepath.Join("..", "..", "..", "testdata", "contract", "runtime_snapshot.codex.golden.json")
	want, err := os.ReadFile(goldenPath)
	if err != nil {
		t.Fatalf("read golden %s: %v", goldenPath, err)
	}

	if strings.TrimSpace(string(got)) != strings.TrimSpace(string(want)) {
		t.Fatalf("golden mismatch\nwant:\n%s\n\ngot:\n%s", string(want), string(got))
	}
}

func normalizeSnapshotJSON(snapshot RuntimeSnapshot) ([]byte, error) {
	snapshot.SourceInstanceID = "test-source-instance"
	return json.MarshalIndent(snapshot, "", "  ")
}
