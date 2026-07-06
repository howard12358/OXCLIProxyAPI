package embedded

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

func TestResolveStateDirUsesExplicitDirectory(t *testing.T) {
	t.Parallel()

	dir, err := ResolveStateDir(filepath.Join(t.TempDir(), "state"))
	if err != nil {
		t.Fatalf("ResolveStateDir() error = %v", err)
	}
	if !filepath.IsAbs(dir) {
		t.Fatalf("ResolveStateDir() = %q, want absolute path", dir)
	}
}

func TestResolveStateDirDefaultsToExecutableDirectory(t *testing.T) {
	t.Parallel()

	executablePath, err := os.Executable()
	if err != nil {
		t.Fatalf("Executable() error = %v", err)
	}
	resolvedExecutablePath, err := filepath.EvalSymlinks(executablePath)
	if err != nil {
		resolvedExecutablePath = executablePath
	}

	dir, err := ResolveStateDir("")
	if err != nil {
		t.Fatalf("ResolveStateDir() error = %v", err)
	}
	if dir != filepath.Dir(resolvedExecutablePath) {
		t.Fatalf("ResolveStateDir() = %q, want %q", dir, filepath.Dir(resolvedExecutablePath))
	}
}

func TestMaterializeArtifactReusesMatchingBinary(t *testing.T) {
	t.Parallel()

	stateDir := t.TempDir()
	artifact := &Artifact{
		FileName: "cliproxy-data-plane",
		Version:  "v1",
		Bytes:    []byte("binary-v1"),
	}

	path, reused, err := MaterializeArtifact(stateDir, artifact)
	if err != nil {
		t.Fatalf("MaterializeArtifact() first call error = %v", err)
	}
	if reused {
		t.Fatal("MaterializeArtifact() first call reused = true, want false")
	}

	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("ReadFile(%q) error = %v", path, err)
	}
	if string(got) != "binary-v1" {
		t.Fatalf("binary content = %q, want %q", string(got), "binary-v1")
	}

	_, reused, err = MaterializeArtifact(stateDir, artifact)
	if err != nil {
		t.Fatalf("MaterializeArtifact() second call error = %v", err)
	}
	if !reused {
		t.Fatal("MaterializeArtifact() second call reused = false, want true")
	}
}

func TestMaterializeArtifactReplacesChangedBinary(t *testing.T) {
	t.Parallel()

	stateDir := t.TempDir()
	first := &Artifact{
		FileName: "cliproxy-data-plane",
		Version:  "v1",
		Bytes:    []byte("binary-v1"),
	}
	second := &Artifact{
		FileName: "cliproxy-data-plane",
		Version:  "v2",
		Bytes:    []byte("binary-v2"),
	}

	path, _, err := MaterializeArtifact(stateDir, first)
	if err != nil {
		t.Fatalf("MaterializeArtifact(first) error = %v", err)
	}
	path, reused, err := MaterializeArtifact(stateDir, second)
	if err != nil {
		t.Fatalf("MaterializeArtifact(second) error = %v", err)
	}
	if reused {
		t.Fatal("MaterializeArtifact(second) reused = true, want false")
	}

	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("ReadFile(%q) error = %v", path, err)
	}
	if string(got) != "binary-v2" {
		t.Fatalf("binary content = %q, want %q", string(got), "binary-v2")
	}
}

func TestDefaultArtifactProviderUsesEnvBinaryPath(t *testing.T) {
	binaryPath := filepath.Join(t.TempDir(), "cliproxy-data-plane")
	if err := os.WriteFile(binaryPath, []byte("binary-v1"), 0o755); err != nil {
		t.Fatalf("WriteFile(%q) error = %v", binaryPath, err)
	}
	t.Setenv("CLIPROXY_DATA_PLANE_BINARY_PATH", binaryPath)

	artifact, err := DefaultArtifactProvider{}.Resolve(context.Background())
	if err != nil {
		t.Fatalf("Resolve() error = %v", err)
	}
	if artifact.FileName != "cliproxy-data-plane" {
		t.Fatalf("artifact.FileName = %q, want %q", artifact.FileName, "cliproxy-data-plane")
	}
	if string(artifact.Bytes) != "binary-v1" {
		t.Fatalf("artifact bytes = %q, want %q", string(artifact.Bytes), "binary-v1")
	}
}
