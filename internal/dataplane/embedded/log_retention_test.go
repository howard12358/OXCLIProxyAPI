package embedded

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestEnforceEmbeddedLogDirSizeLimitKeepsActiveLogsAndDeletesOldRotated(t *testing.T) {
	t.Parallel()

	dir := t.TempDir()
	activeStdout := filepath.Join(dir, "stdout.log")
	activeStderr := filepath.Join(dir, "stderr.log")
	oldRotated := filepath.Join(dir, "stdout-20260707T010203.log")
	newRotated := filepath.Join(dir, "stderr-20260707T020304.log")

	writeSizedFile(t, activeStdout, 10)
	writeSizedFile(t, activeStderr, 10)
	writeSizedFile(t, oldRotated, 30)
	writeSizedFile(t, newRotated, 30)

	oldTime := time.Now().Add(-2 * time.Hour)
	newTime := time.Now().Add(-1 * time.Hour)
	if err := os.Chtimes(oldRotated, oldTime, oldTime); err != nil {
		t.Fatalf("chtimes old rotated: %v", err)
	}
	if err := os.Chtimes(newRotated, newTime, newTime); err != nil {
		t.Fatalf("chtimes new rotated: %v", err)
	}

	deleted, err := enforceEmbeddedLogDirSizeLimit(dir, 50)
	if err != nil {
		t.Fatalf("enforceEmbeddedLogDirSizeLimit() error = %v", err)
	}
	if deleted != 1 {
		t.Fatalf("deleted = %d, want 1", deleted)
	}
	if _, err := os.Stat(activeStdout); err != nil {
		t.Fatalf("stdout.log missing: %v", err)
	}
	if _, err := os.Stat(activeStderr); err != nil {
		t.Fatalf("stderr.log missing: %v", err)
	}
	if _, err := os.Stat(oldRotated); !os.IsNotExist(err) {
		t.Fatalf("old rotated stat err = %v, want not exist", err)
	}
	if _, err := os.Stat(newRotated); err != nil {
		t.Fatalf("new rotated missing: %v", err)
	}
}

func writeSizedFile(t *testing.T, path string, size int) {
	t.Helper()
	data := make([]byte, size)
	for i := range data {
		data[i] = 'x'
	}
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatalf("write file %s: %v", path, err)
	}
}
