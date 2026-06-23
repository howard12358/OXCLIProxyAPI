package embedded

import (
	"bytes"
	"context"
	"errors"
	"io"
	"testing"
	"time"

	log "github.com/sirupsen/logrus"
)

type fakeArtifactProvider struct {
	artifact *Artifact
	err      error
}

func (p fakeArtifactProvider) Resolve(context.Context) (*Artifact, error) {
	if p.err != nil {
		return nil, p.err
	}
	return p.artifact, nil
}

type fakeProcess struct {
	pid      int
	waitCh   chan error
	killErr  error
	killHits int
}

func (p *fakeProcess) PID() int { return p.pid }

func (p *fakeProcess) Wait() error { return <-p.waitCh }

func (p *fakeProcess) Kill() error {
	p.killHits++
	if p.killErr != nil {
		return p.killErr
	}
	select {
	case p.waitCh <- nil:
	default:
	}
	return nil
}

type fakeLauncher struct {
	process *fakeProcess
	err     error
}

func (l fakeLauncher) Start(context.Context, LaunchSpec) (Process, error) {
	if l.err != nil {
		return nil, l.err
	}
	return l.process, nil
}

func TestSupervisorStartSetsEffectiveBaseURLAndStatus(t *testing.T) {
	t.Parallel()

	process := &fakeProcess{pid: 42, waitCh: make(chan error, 1)}
	supervisor := NewSupervisor(SupervisorConfig{
		BindAddr:            "127.0.0.1:4100",
		StateDir:            t.TempDir(),
		LogLevel:            "info",
		SnapshotURL:         "http://127.0.0.1:8317/v0/management/runtime-snapshot",
		SnapshotBearerToken: "test-token",
		SnapshotPollSeconds: 30,
		StartupTimeout:      time.Second,
		ArtifactProvider:    fakeArtifactProvider{artifact: &Artifact{FileName: "cliproxy-data-plane", Version: "v1", Bytes: []byte("test-binary")}},
		Launcher:            fakeLauncher{process: process},
		ReadinessChecker:    func(context.Context, string) error { return nil },
		StdoutWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
		StderrWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
	})

	if err := supervisor.Start(context.Background()); err != nil {
		t.Fatalf("Start() error = %v", err)
	}

	if got := supervisor.EffectiveBaseURL(); got != "http://127.0.0.1:4100" {
		t.Fatalf("EffectiveBaseURL() = %q, want %q", got, "http://127.0.0.1:4100")
	}

	status := supervisor.Status()
	if status.PID != 42 {
		t.Fatalf("status.PID = %d, want 42", status.PID)
	}
	if status.ArtifactVersion != "v1" {
		t.Fatalf("status.ArtifactVersion = %q, want %q", status.ArtifactVersion, "v1")
	}
	if status.LastReadyTime.IsZero() {
		t.Fatal("status.LastReadyTime is zero, want ready timestamp")
	}
}

func TestSupervisorStopKillsProcess(t *testing.T) {
	t.Parallel()

	process := &fakeProcess{pid: 7, waitCh: make(chan error, 1)}
	supervisor := NewSupervisor(SupervisorConfig{
		BindAddr:            "127.0.0.1:4101",
		StateDir:            t.TempDir(),
		LogLevel:            "info",
		SnapshotURL:         "http://127.0.0.1:8317/v0/management/runtime-snapshot",
		SnapshotBearerToken: "test-token",
		SnapshotPollSeconds: 30,
		StartupTimeout:      time.Second,
		ArtifactProvider:    fakeArtifactProvider{artifact: &Artifact{FileName: "cliproxy-data-plane", Version: "v1", Bytes: []byte("test-binary")}},
		Launcher:            fakeLauncher{process: process},
		ReadinessChecker:    func(context.Context, string) error { return nil },
		StdoutWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
		StderrWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
	})

	if err := supervisor.Start(context.Background()); err != nil {
		t.Fatalf("Start() error = %v", err)
	}
	if err := supervisor.Stop(context.Background()); err != nil {
		t.Fatalf("Stop() error = %v", err)
	}
	if process.killHits != 1 {
		t.Fatalf("process.killHits = %d, want 1", process.killHits)
	}
}

func TestSupervisorStartReturnsReadinessError(t *testing.T) {
	t.Parallel()

	process := &fakeProcess{pid: 99, waitCh: make(chan error, 1)}
	supervisor := NewSupervisor(SupervisorConfig{
		BindAddr:            "127.0.0.1:4102",
		StateDir:            t.TempDir(),
		LogLevel:            "info",
		SnapshotURL:         "http://127.0.0.1:8317/v0/management/runtime-snapshot",
		SnapshotBearerToken: "test-token",
		SnapshotPollSeconds: 30,
		StartupTimeout:      time.Second,
		ArtifactProvider:    fakeArtifactProvider{artifact: &Artifact{FileName: "cliproxy-data-plane", Version: "v1", Bytes: []byte("test-binary")}},
		Launcher:            fakeLauncher{process: process},
		ReadinessChecker:    func(context.Context, string) error { return errors.New("not ready") },
		StdoutWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
		StderrWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
	})

	err := supervisor.Start(context.Background())
	if err == nil || err.Error() != "wait for embedded data plane readiness: not ready" {
		t.Fatalf("Start() error = %v, want readiness failure", err)
	}
}

func TestSupervisorLogsLifecycle(t *testing.T) {
	var buf bytes.Buffer
	originalOutput := log.StandardLogger().Out
	originalFormatter := log.StandardLogger().Formatter
	originalLevel := log.StandardLogger().Level
	log.SetOutput(&buf)
	log.SetFormatter(&log.TextFormatter{
		DisableTimestamp: true,
		DisableColors:    true,
	})
	log.SetLevel(log.InfoLevel)
	defer func() {
		log.SetOutput(originalOutput)
		log.SetFormatter(originalFormatter)
		log.SetLevel(originalLevel)
	}()

	process := &fakeProcess{pid: 88, waitCh: make(chan error, 1)}
	supervisor := NewSupervisor(SupervisorConfig{
		BindAddr:            "127.0.0.1:4110",
		StateDir:            t.TempDir(),
		LogLevel:            "info",
		SnapshotURL:         "http://127.0.0.1:8317/v0/management/runtime-snapshot",
		SnapshotBearerToken: "test-token",
		SnapshotPollSeconds: 30,
		StartupTimeout:      time.Second,
		ArtifactProvider:    fakeArtifactProvider{artifact: &Artifact{FileName: "cliproxy-data-plane", Version: "v1", Bytes: []byte("test-binary")}},
		Launcher:            fakeLauncher{process: process},
		ReadinessChecker:    func(context.Context, string) error { return nil },
		StdoutWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
		StderrWriterFactory: func(string) (io.WriteCloser, error) { return nopWriteCloser{Writer: io.Discard}, nil },
	})

	if err := supervisor.Start(context.Background()); err != nil {
		t.Fatalf("Start() error = %v", err)
	}
	if err := supervisor.Stop(context.Background()); err != nil {
		t.Fatalf("Stop() error = %v", err)
	}

	output := buf.String()
	for _, needle := range []string{
		"starting embedded data plane",
		"embedded data plane started",
		"embedded data plane ready",
		"stopping embedded data plane",
	} {
		if !bytes.Contains([]byte(output), []byte(needle)) {
			t.Fatalf("log output missing %q:\n%s", needle, output)
		}
	}
}
