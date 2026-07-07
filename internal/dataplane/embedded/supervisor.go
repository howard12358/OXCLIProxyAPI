package embedded

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	log "github.com/sirupsen/logrus"
)

type LaunchSpec struct {
	BinaryPath string
	Args       []string
	Stdout     io.Writer
	Stderr     io.Writer
}

type Process interface {
	PID() int
	Wait() error
	Kill() error
}

type ProcessLauncher interface {
	Start(context.Context, LaunchSpec) (Process, error)
}

type ReadinessChecker func(context.Context, string) error

type StdWriterFactory func(string) (io.WriteCloser, error)

type SupervisorConfig struct {
	BindAddr            string
	StateDir            string
	LogLevel            string
	SnapshotURL         string
	SnapshotBearerToken string
	SnapshotPollSeconds int
	StartupTimeout      time.Duration
	ArtifactProvider    ArtifactProvider
	Launcher            ProcessLauncher
	ReadinessChecker    ReadinessChecker
	StdoutWriterFactory StdWriterFactory
	StderrWriterFactory StdWriterFactory
}

type SupervisorStatus struct {
	PID             int
	BindAddr        string
	BaseURL         string
	ArtifactVersion string
	ArtifactSHA256  string
	LastStartTime   time.Time
	LastReadyTime   time.Time
	LastExitReason  string
}

type Supervisor struct {
	cfg SupervisorConfig

	mu           sync.Mutex
	process      Process
	stdoutCloser io.Closer
	stderrCloser io.Closer
	logCleaner   *logCleaner
	status       SupervisorStatus
}

func NewSupervisor(cfg SupervisorConfig) *Supervisor {
	if cfg.ArtifactProvider == nil {
		cfg.ArtifactProvider = DefaultArtifactProvider{}
	}
	if cfg.Launcher == nil {
		cfg.Launcher = osExecProcessLauncher{}
	}
	if cfg.ReadinessChecker == nil {
		cfg.ReadinessChecker = HTTPReadinessChecker
	}
	if cfg.StdoutWriterFactory == nil {
		cfg.StdoutWriterFactory = defaultStdoutWriterFactory
	}
	if cfg.StderrWriterFactory == nil {
		cfg.StderrWriterFactory = defaultStderrWriterFactory
	}
	if cfg.StartupTimeout <= 0 {
		cfg.StartupTimeout = 20 * time.Second
	}
	if cfg.SnapshotPollSeconds <= 0 {
		cfg.SnapshotPollSeconds = 30
	}
	return &Supervisor{cfg: cfg}
}

func (s *Supervisor) Start(ctx context.Context) error {
	if s == nil {
		return nil
	}

	s.mu.Lock()
	if s.process != nil {
		s.mu.Unlock()
		return nil
	}
	s.mu.Unlock()

	artifact, err := s.cfg.ArtifactProvider.Resolve(ctx)
	if err != nil {
		return fmt.Errorf("resolve embedded data plane artifact: %w", err)
	}
	stateDir, err := ResolveStateDir(s.cfg.StateDir)
	if err != nil {
		return err
	}
	log.WithFields(log.Fields{
		"bind_addr":    strings.TrimSpace(s.cfg.BindAddr),
		"state_dir":    stateDir,
		"snapshot_url": strings.TrimSpace(s.cfg.SnapshotURL),
	}).Info("starting embedded data plane")
	binaryPath, _, err := MaterializeArtifact(stateDir, artifact)
	if err != nil {
		return err
	}
	logDir := filepath.Join(stateDir, "logs", embeddedLogSubdir)
	if err := os.MkdirAll(logDir, 0o755); err != nil {
		return fmt.Errorf("create embedded data plane log dir %s: %w", logDir, err)
	}
	cleaner := startEmbeddedLogCleaner(logDir)

	stdoutWriter, err := s.cfg.StdoutWriterFactory(filepath.Join(logDir, "stdout.log"))
	if err != nil {
		if cleaner != nil {
			cleaner.Stop()
		}
		return fmt.Errorf("open embedded data plane stdout log: %w", err)
	}
	stderrWriter, err := s.cfg.StderrWriterFactory(filepath.Join(logDir, "stderr.log"))
	if err != nil {
		_ = stdoutWriter.Close()
		if cleaner != nil {
			cleaner.Stop()
		}
		return fmt.Errorf("open embedded data plane stderr log: %w", err)
	}

	baseURL := "http://" + strings.TrimSpace(s.cfg.BindAddr)
	args := []string{
		"--bind-addr", strings.TrimSpace(s.cfg.BindAddr),
		"--snapshot-url", strings.TrimSpace(s.cfg.SnapshotURL),
		"--snapshot-poll-seconds", fmt.Sprintf("%d", s.cfg.SnapshotPollSeconds),
		"--log-level", strings.TrimSpace(s.cfg.LogLevel),
	}
	if token := strings.TrimSpace(s.cfg.SnapshotBearerToken); token != "" {
		args = append(args, "--snapshot-bearer-token", token)
	}

	process, err := s.cfg.Launcher.Start(ctx, LaunchSpec{
		BinaryPath: binaryPath,
		Args:       args,
		Stdout:     stdoutWriter,
		Stderr:     stderrWriter,
	})
	if err != nil {
		_ = stdoutWriter.Close()
		_ = stderrWriter.Close()
		if cleaner != nil {
			cleaner.Stop()
		}
		return fmt.Errorf("start embedded data plane process: %w", err)
	}

	now := time.Now().UTC()
	s.mu.Lock()
	s.process = process
	s.stdoutCloser = stdoutWriter
	s.stderrCloser = stderrWriter
	s.logCleaner = cleaner
	s.status = SupervisorStatus{
		PID:             process.PID(),
		BindAddr:        strings.TrimSpace(s.cfg.BindAddr),
		BaseURL:         baseURL,
		ArtifactVersion: strings.TrimSpace(artifact.Version),
		ArtifactSHA256:  artifact.FileSHA256(),
		LastStartTime:   now,
	}
	s.mu.Unlock()
	log.WithFields(log.Fields{
		"pid":       process.PID(),
		"base_url":  baseURL,
		"bind_addr": strings.TrimSpace(s.cfg.BindAddr),
	}).Info("embedded data plane started")

	go s.waitForExit(process)

	readinessCtx, cancel := context.WithTimeout(context.Background(), s.cfg.StartupTimeout)
	defer cancel()
	if err := s.cfg.ReadinessChecker(readinessCtx, baseURL); err != nil {
		_ = s.Stop(context.Background())
		return fmt.Errorf("wait for embedded data plane readiness: %w", err)
	}

	s.mu.Lock()
	s.status.LastReadyTime = time.Now().UTC()
	s.mu.Unlock()
	log.WithFields(log.Fields{
		"pid":      process.PID(),
		"base_url": baseURL,
	}).Info("embedded data plane ready")
	return nil
}

func (s *Supervisor) Stop(context.Context) error {
	if s == nil {
		return nil
	}

	s.mu.Lock()
	process := s.process
	s.mu.Unlock()
	if process == nil {
		return nil
	}
	log.WithFields(log.Fields{
		"pid":      process.PID(),
		"base_url": s.EffectiveBaseURL(),
	}).Info("stopping embedded data plane")

	err := process.Kill()
	s.mu.Lock()
	s.process = nil
	if s.stdoutCloser != nil {
		_ = s.stdoutCloser.Close()
		s.stdoutCloser = nil
	}
	if s.stderrCloser != nil {
		_ = s.stderrCloser.Close()
		s.stderrCloser = nil
	}
	if s.logCleaner != nil {
		s.logCleaner.Stop()
		s.logCleaner = nil
	}
	s.mu.Unlock()
	if err != nil {
		return fmt.Errorf("stop embedded data plane process: %w", err)
	}
	return nil
}

func (s *Supervisor) EffectiveBaseURL() string {
	if s == nil {
		return ""
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.status.BaseURL
}

func (s *Supervisor) Status() SupervisorStatus {
	if s == nil {
		return SupervisorStatus{}
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.status
}

func (s *Supervisor) waitForExit(process Process) {
	err := process.Wait()

	s.mu.Lock()
	defer s.mu.Unlock()
	if s.process != process {
		return
	}
	s.process = nil
	if s.stdoutCloser != nil {
		_ = s.stdoutCloser.Close()
		s.stdoutCloser = nil
	}
	if s.stderrCloser != nil {
		_ = s.stderrCloser.Close()
		s.stderrCloser = nil
	}
	if s.logCleaner != nil {
		s.logCleaner.Stop()
		s.logCleaner = nil
	}
	if err != nil {
		s.status.LastExitReason = err.Error()
	} else {
		s.status.LastExitReason = "process exited"
	}
	log.WithFields(log.Fields{
		"pid":    process.PID(),
		"reason": s.status.LastExitReason,
	}).Info("embedded data plane stopped")
}

type nopWriteCloser struct {
	io.Writer
}

func (nopWriteCloser) Close() error { return nil }

type osExecProcessLauncher struct{}

func (osExecProcessLauncher) Start(ctx context.Context, spec LaunchSpec) (Process, error) {
	cmd := exec.CommandContext(ctx, spec.BinaryPath, spec.Args...)
	cmd.Stdout = spec.Stdout
	cmd.Stderr = spec.Stderr
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	return execProcess{cmd: cmd}, nil
}

type execProcess struct {
	cmd *exec.Cmd
}

func (p execProcess) PID() int {
	if p.cmd == nil || p.cmd.Process == nil {
		return 0
	}
	return p.cmd.Process.Pid
}

func (p execProcess) Wait() error {
	if p.cmd == nil {
		return nil
	}
	return p.cmd.Wait()
}

func (p execProcess) Kill() error {
	if p.cmd == nil || p.cmd.Process == nil {
		return nil
	}
	return p.cmd.Process.Kill()
}

func HTTPReadinessChecker(ctx context.Context, baseURL string) error {
	client := &http.Client{
		Transport: &http.Transport{Proxy: nil},
	}
	readyURL := strings.TrimRight(baseURL, "/") + "/readyz"
	ticker := time.NewTicker(200 * time.Millisecond)
	defer ticker.Stop()

	for {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, readyURL, nil)
		if err != nil {
			return err
		}
		resp, err := client.Do(req)
		if err == nil {
			if resp.Body != nil {
				_ = resp.Body.Close()
			}
			if resp.StatusCode == http.StatusOK {
				return nil
			}
		}

		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
		}
	}
}
