package notifier

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	dataplanesnapshot "github.com/router-for-me/CLIProxyAPI/v7/internal/dataplane/snapshot"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
	log "github.com/sirupsen/logrus"
)

const snapshotNotifyPath = "/v0/runtime/snapshot-notify"

type snapshotNotifyRequest struct {
	Version     string `json:"version"`
	GeneratedAt string `json:"generated_at"`
	Reason      string `json:"reason"`
}

// RuntimeSnapshotNotifier sends best-effort snapshot change notifications to the Rust data plane.
type RuntimeSnapshotNotifier struct {
	client *http.Client
	mu     sync.Mutex

	lastVersion string
}

// NewRuntimeSnapshotNotifier creates a notifier seeded with the current runtime snapshot version.
func NewRuntimeSnapshotNotifier(cfg *config.Config, authManager *coreauth.Manager) *RuntimeSnapshotNotifier {
	notifier := &RuntimeSnapshotNotifier{
		client: &http.Client{
			Transport: &http.Transport{Proxy: nil},
		},
	}
	if cfg != nil {
		notifier.lastVersion = currentSnapshotVersion(cfg.CloneForRuntime(), authManager)
	}
	return notifier
}

// NotifyIfChanged emits a notify request only when the effective snapshot version changes.
func (n *RuntimeSnapshotNotifier) NotifyIfChanged(ctx context.Context, cfg *config.Config, authManager *coreauth.Manager) error {
	if n == nil || cfg == nil {
		return nil
	}

	targetURL, ok := snapshotNotifyURL(cfg.DataPlane.EffectiveResponsesBaseURL())
	if !ok {
		return nil
	}

	snapshot := dataplanesnapshot.BuildRuntimeSnapshot(cfg, authManager, time.Now().UTC())
	if strings.TrimSpace(snapshot.Version) == "" {
		return nil
	}

	n.mu.Lock()
	unchanged := n.lastVersion == snapshot.Version
	n.mu.Unlock()
	if unchanged {
		return nil
	}

	requestBody, errMarshal := json.Marshal(snapshotNotifyRequest{
		Version:     snapshot.Version,
		GeneratedAt: snapshot.GeneratedAt,
		Reason:      "runtime_snapshot_changed",
	})
	if errMarshal != nil {
		return fmt.Errorf("marshal snapshot notify request: %w", errMarshal)
	}

	if ctx == nil {
		ctx = context.Background()
	}
	req, errRequest := http.NewRequestWithContext(ctx, http.MethodPost, targetURL, bytes.NewReader(requestBody))
	if errRequest != nil {
		return fmt.Errorf("build snapshot notify request: %w", errRequest)
	}
	req.Header.Set("Content-Type", "application/json")

	resp, errDo := n.client.Do(req)
	if errDo != nil {
		return fmt.Errorf("send snapshot notify request: %w", errDo)
	}
	defer func() {
		if errClose := resp.Body.Close(); errClose != nil {
			log.WithError(errClose).Debug("failed to close snapshot notify response body")
		}
	}()

	if resp.StatusCode < http.StatusOK || resp.StatusCode >= http.StatusMultipleChoices {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 1024))
		return fmt.Errorf("snapshot notify returned status %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}

	n.mu.Lock()
	n.lastVersion = snapshot.Version
	n.mu.Unlock()
	return nil
}

func currentSnapshotVersion(cfg *config.Config, authManager *coreauth.Manager) string {
	if cfg == nil {
		return ""
	}
	return dataplanesnapshot.BuildRuntimeSnapshot(cfg, authManager, time.Now().UTC()).Version
}

func snapshotNotifyURL(baseURL string) (string, bool) {
	baseURL = strings.TrimSpace(baseURL)
	if baseURL == "" {
		return "", false
	}

	target, errParse := url.Parse(baseURL)
	if errParse != nil || strings.TrimSpace(target.Scheme) == "" || strings.TrimSpace(target.Host) == "" {
		return "", false
	}

	target.Path = snapshotNotifyPath
	target.RawPath = ""
	target.RawQuery = ""
	target.Fragment = ""
	return target.String(), true
}
