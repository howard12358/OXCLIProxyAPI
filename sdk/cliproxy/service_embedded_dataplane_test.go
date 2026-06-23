package cliproxy

import (
	"context"
	"testing"
	"time"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/dataplane/embedded"
	"github.com/router-for-me/CLIProxyAPI/v7/sdk/config"
)

type fakeEmbeddedDataPlaneController struct {
	baseURL    string
	startCalls int
	stopCalls  int
}

func (f *fakeEmbeddedDataPlaneController) Start(context.Context) error {
	f.startCalls++
	return nil
}

func (f *fakeEmbeddedDataPlaneController) Stop(context.Context) error {
	f.stopCalls++
	return nil
}

func (f *fakeEmbeddedDataPlaneController) EffectiveBaseURL() string {
	return f.baseURL
}

func testEmbeddedConfig() *config.Config {
	return &config.Config{
		Host: "127.0.0.1",
		Port: 8318,
		DataPlane: config.DataPlaneConfig{
			Mode: "embedded",
			Embedded: config.EmbeddedDataPlaneConfig{
				Enabled:               true,
				BindAddr:              "127.0.0.1:4101",
				StateDir:              "/tmp/embedded-data-plane",
				LogLevel:              "info",
				StartupTimeoutSeconds: 20,
			},
		},
	}
}

func TestReconcileEmbeddedDataPlaneSkipsRestartWhenBootstrapConfigUnchanged(t *testing.T) {
	cfg := testEmbeddedConfig()
	service := &Service{
		cfg: cfg,
		embeddedDataPlaneFactory: func(cfg embedded.SupervisorConfig) embeddedDataPlaneController {
			return &fakeEmbeddedDataPlaneController{baseURL: "http://" + cfg.BindAddr}
		},
	}

	if err := service.reconcileEmbeddedDataPlane(context.Background(), cfg); err != nil {
		t.Fatalf("first reconcile error = %v", err)
	}
	firstController, ok := service.embeddedDataPlane.(*fakeEmbeddedDataPlaneController)
	if !ok {
		t.Fatalf("embeddedDataPlane = %T, want *fakeEmbeddedDataPlaneController", service.embeddedDataPlane)
	}

	cfgReload := cfg.CloneForRuntime()
	if err := service.reconcileEmbeddedDataPlane(context.Background(), cfgReload); err != nil {
		t.Fatalf("second reconcile error = %v", err)
	}
	secondController, ok := service.embeddedDataPlane.(*fakeEmbeddedDataPlaneController)
	if !ok {
		t.Fatalf("embeddedDataPlane = %T, want *fakeEmbeddedDataPlaneController", service.embeddedDataPlane)
	}

	if firstController != secondController {
		t.Fatal("expected embedded data plane controller to be reused")
	}
	if firstController.stopCalls != 0 {
		t.Fatalf("stopCalls = %d, want 0", firstController.stopCalls)
	}
	if got := cfgReload.DataPlane.RuntimeResponsesBaseURL; got != "http://127.0.0.1:4101" {
		t.Fatalf("RuntimeResponsesBaseURL = %q, want %q", got, "http://127.0.0.1:4101")
	}
}

func TestReconcileEmbeddedDataPlaneRestartsWhenBootstrapConfigChanges(t *testing.T) {
	cfg := testEmbeddedConfig()
	var created []*fakeEmbeddedDataPlaneController
	service := &Service{
		cfg: cfg,
		embeddedDataPlaneFactory: func(cfg embedded.SupervisorConfig) embeddedDataPlaneController {
			controller := &fakeEmbeddedDataPlaneController{baseURL: "http://" + cfg.BindAddr}
			created = append(created, controller)
			return controller
		},
	}

	if err := service.reconcileEmbeddedDataPlane(context.Background(), cfg); err != nil {
		t.Fatalf("first reconcile error = %v", err)
	}

	cfgReload := cfg.CloneForRuntime()
	cfgReload.DataPlane.Embedded.BindAddr = "127.0.0.1:4102"
	cfgReload.DataPlane.Embedded.StartupTimeoutSeconds = int((25 * time.Second) / time.Second)
	if err := service.reconcileEmbeddedDataPlane(context.Background(), cfgReload); err != nil {
		t.Fatalf("second reconcile error = %v", err)
	}

	if len(created) != 2 {
		t.Fatalf("created controllers = %d, want 2", len(created))
	}
	if created[0].stopCalls != 1 {
		t.Fatalf("first controller stopCalls = %d, want 1", created[0].stopCalls)
	}
	if got := cfgReload.DataPlane.RuntimeResponsesBaseURL; got != "http://127.0.0.1:4102" {
		t.Fatalf("RuntimeResponsesBaseURL = %q, want %q", got, "http://127.0.0.1:4102")
	}
}
