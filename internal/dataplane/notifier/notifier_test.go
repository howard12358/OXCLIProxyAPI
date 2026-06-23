package notifier

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
)

func TestRuntimeSnapshotNotifierPostsOnlyWhenSnapshotVersionChanges(t *testing.T) {
	var calls atomic.Int32
	var lastPath atomic.Value
	var lastBody atomic.Value

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		lastPath.Store(r.URL.Path)
		defer r.Body.Close()
		var payload map[string]any
		if errDecode := json.NewDecoder(r.Body).Decode(&payload); errDecode != nil {
			t.Fatalf("decode payload: %v", errDecode)
		}
		lastBody.Store(payload)
		w.WriteHeader(http.StatusAccepted)
	}))
	defer server.Close()

	cfg := &config.Config{}
	cfg.DataPlane.ResponsesBaseURL = server.URL
	cfg.ProxyURL = "socks5h://127.0.0.1:7897"

	manager := auth.NewManager(nil, nil, nil)
	notifier := NewRuntimeSnapshotNotifier(cfg, manager)

	if err := notifier.NotifyIfChanged(context.Background(), cfg.CloneForRuntime(), manager); err != nil {
		t.Fatalf("NotifyIfChanged(same) error = %v", err)
	}
	if got := calls.Load(); got != 0 {
		t.Fatalf("calls after unchanged notify = %d, want 0", got)
	}

	cfg.ProxyURL = "direct"
	if err := notifier.NotifyIfChanged(context.Background(), cfg.CloneForRuntime(), manager); err != nil {
		t.Fatalf("NotifyIfChanged(changed) error = %v", err)
	}
	if got := calls.Load(); got != 1 {
		t.Fatalf("calls after changed notify = %d, want 1", got)
	}
	if got, _ := lastPath.Load().(string); got != "/v0/runtime/snapshot-notify" {
		t.Fatalf("path = %q, want /v0/runtime/snapshot-notify", got)
	}

	body, _ := lastBody.Load().(map[string]any)
	if got, _ := body["version"].(string); got == "" {
		t.Fatal("expected version in notify payload")
	}
	if got, _ := body["reason"].(string); got != "runtime_snapshot_changed" {
		t.Fatalf("reason = %q, want runtime_snapshot_changed", got)
	}

	if err := notifier.NotifyIfChanged(context.Background(), cfg.CloneForRuntime(), manager); err != nil {
		t.Fatalf("NotifyIfChanged(repeat) error = %v", err)
	}
	if got := calls.Load(); got != 1 {
		t.Fatalf("calls after repeated unchanged notify = %d, want 1", got)
	}
}

func TestRuntimeSnapshotNotifierUsesRuntimeResponsesBaseURLOverride(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		w.WriteHeader(http.StatusAccepted)
	}))
	defer server.Close()

	cfg := &config.Config{}
	cfg.DataPlane.RuntimeResponsesBaseURL = server.URL

	manager := auth.NewManager(nil, nil, nil)
	notifier := &RuntimeSnapshotNotifier{
		client: &http.Client{
			Transport: &http.Transport{Proxy: nil},
		},
	}

	if err := notifier.NotifyIfChanged(context.Background(), cfg.CloneForRuntime(), manager); err != nil {
		t.Fatalf("NotifyIfChanged() error = %v", err)
	}
	if got := calls.Load(); got != 1 {
		t.Fatalf("calls = %d, want 1", got)
	}
}
