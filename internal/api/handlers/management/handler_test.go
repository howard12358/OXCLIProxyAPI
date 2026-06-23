package management

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/gin-gonic/gin"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	"github.com/router-for-me/CLIProxyAPI/v7/internal/pluginhost"
	coreauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
)

type snapshotNotifierStub struct {
	ch chan *config.Config
}

func (s *snapshotNotifierStub) NotifyIfChanged(_ context.Context, cfg *config.Config, _ *coreauth.Manager) error {
	s.ch <- cfg
	return nil
}

func TestAuthenticateManagementKey_LocalhostIPBan_BlocksCorrectKeyDuringBan(t *testing.T) {
	h := &Handler{
		cfg:            &config.Config{},
		failedAttempts: make(map[string]*attemptInfo),
		envSecret:      "test-secret",
	}

	for i := 0; i < 5; i++ {
		allowed, statusCode, errMsg := h.AuthenticateManagementKey("127.0.0.1", true, "wrong-secret")
		if allowed {
			t.Fatalf("expected auth to be denied at attempt %d", i+1)
		}
		if statusCode != http.StatusUnauthorized || errMsg != "invalid management key" {
			t.Fatalf("unexpected auth failure at attempt %d: status=%d msg=%q", i+1, statusCode, errMsg)
		}
	}

	allowed, statusCode, errMsg := h.AuthenticateManagementKey("127.0.0.1", true, "test-secret")
	if allowed {
		t.Fatalf("expected correct key to be denied while banned")
	}
	if statusCode != http.StatusForbidden {
		t.Fatalf("expected forbidden status while banned, got %d", statusCode)
	}
	if !strings.HasPrefix(errMsg, "IP banned due to too many failed attempts. Try again in") {
		t.Fatalf("unexpected banned message: %q", errMsg)
	}
}

func TestMiddlewareSetsSupportPluginHeader(t *testing.T) {

	h := &Handler{
		cfg:            &config.Config{},
		failedAttempts: make(map[string]*attemptInfo),
		envSecret:      "test-secret",
	}
	middleware := h.Middleware()

	t.Run("invalid key", func(t *testing.T) {
		rec := httptest.NewRecorder()
		c, _ := gin.CreateTestContext(rec)
		c.Request = httptest.NewRequest(http.MethodGet, "/v0/management/config", nil)
		c.Request.RemoteAddr = "127.0.0.1:12345"
		c.Request.Header.Set("X-Management-Key", "wrong-secret")

		middleware(c)

		if rec.Code != http.StatusUnauthorized {
			t.Fatalf("status = %d, want %d", rec.Code, http.StatusUnauthorized)
		}
		if got := rec.Header().Get("X-CPA-SUPPORT-PLUGIN"); got != pluginhost.SupportPluginHeaderValue() {
			t.Fatalf("X-CPA-SUPPORT-PLUGIN = %q, want %q", got, pluginhost.SupportPluginHeaderValue())
		}
	})

	t.Run("valid key", func(t *testing.T) {
		engine := gin.New()
		engine.GET("/v0/management/config", middleware, func(c *gin.Context) {
			c.Status(http.StatusOK)
		})

		rec := httptest.NewRecorder()
		req := httptest.NewRequest(http.MethodGet, "/v0/management/config", nil)
		req.RemoteAddr = "127.0.0.1:12345"
		req.Header.Set("X-Management-Key", "test-secret")
		engine.ServeHTTP(rec, req)

		if rec.Code != http.StatusOK {
			t.Fatalf("status = %d, want %d", rec.Code, http.StatusOK)
		}
		if got := rec.Header().Get("X-CPA-SUPPORT-PLUGIN"); got != pluginhost.SupportPluginHeaderValue() {
			t.Fatalf("X-CPA-SUPPORT-PLUGIN = %q, want %q", got, pluginhost.SupportPluginHeaderValue())
		}
	})
}

func TestPutProxyURLTriggersSnapshotNotify(t *testing.T) {
	gin.SetMode(gin.TestMode)

	notifier := &snapshotNotifierStub{ch: make(chan *config.Config, 1)}
	configFile, errTemp := os.CreateTemp(t.TempDir(), "config-*.yaml")
	if errTemp != nil {
		t.Fatalf("CreateTemp() error = %v", errTemp)
	}
	if errClose := configFile.Close(); errClose != nil {
		t.Fatalf("Close() error = %v", errClose)
	}
	if errWrite := os.WriteFile(configFile.Name(), []byte("proxy-url: \"\"\n"), 0o644); errWrite != nil {
		t.Fatalf("WriteFile() error = %v", errWrite)
	}
	h := &Handler{
		cfg:              &config.Config{},
		configFilePath:   configFile.Name(),
		snapshotNotifier: notifier,
		failedAttempts:   make(map[string]*attemptInfo),
	}

	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	c.Request = httptest.NewRequest(http.MethodPut, "/v0/management/proxy-url", strings.NewReader(`{"value":"socks5h://127.0.0.1:7897"}`))
	c.Request.Header.Set("content-type", "application/json")

	h.PutProxyURL(c)

	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", rec.Code, http.StatusOK)
	}

	select {
	case cfg := <-notifier.ch:
		if cfg == nil {
			t.Fatal("notify cfg = nil")
		}
		if cfg.ProxyURL != "socks5h://127.0.0.1:7897" {
			t.Fatalf("notify cfg.ProxyURL = %q, want socks5h://127.0.0.1:7897", cfg.ProxyURL)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("expected snapshot notify to be triggered")
	}
}

func TestPutConfigYAMLTriggersSnapshotNotify(t *testing.T) {
	gin.SetMode(gin.TestMode)

	notifier := &snapshotNotifierStub{ch: make(chan *config.Config, 1)}
	configFile, errTemp := os.CreateTemp(t.TempDir(), "config-*.yaml")
	if errTemp != nil {
		t.Fatalf("CreateTemp() error = %v", errTemp)
	}
	if errClose := configFile.Close(); errClose != nil {
		t.Fatalf("Close() error = %v", errClose)
	}
	if errWrite := os.WriteFile(configFile.Name(), []byte("proxy-url: \"http://127.0.0.1:7897\"\n"), 0o644); errWrite != nil {
		t.Fatalf("WriteFile() error = %v", errWrite)
	}
	h := &Handler{
		cfg:              &config.Config{},
		configFilePath:   configFile.Name(),
		snapshotNotifier: notifier,
		failedAttempts:   make(map[string]*attemptInfo),
	}

	rec := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(rec)
	c.Request = httptest.NewRequest(http.MethodPut, "/v0/management/config.yaml", strings.NewReader("proxy-url: \"socks5h://127.0.0.1:7897\"\n"))
	c.Request.Header.Set("content-type", "application/yaml")

	h.PutConfigYAML(c)

	if rec.Code != http.StatusOK {
		t.Fatalf("status = %d, want %d", rec.Code, http.StatusOK)
	}

	select {
	case cfg := <-notifier.ch:
		if cfg == nil {
			t.Fatal("notify cfg = nil")
		}
		if cfg.ProxyURL != "socks5h://127.0.0.1:7897" {
			t.Fatalf("notify cfg.ProxyURL = %q, want socks5h://127.0.0.1:7897", cfg.ProxyURL)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("expected snapshot notify to be triggered")
	}
}
