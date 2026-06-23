package embedded

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/util"
)

var ErrEmbeddedArtifactUnavailable = errors.New("embedded data plane artifact is unavailable")

type Artifact struct {
	FileName string
	Version  string
	SHA256   string
	Bytes    []byte
}

func (a *Artifact) FileSHA256() string {
	if a == nil {
		return ""
	}
	if strings.TrimSpace(a.SHA256) != "" {
		return strings.TrimSpace(a.SHA256)
	}
	sum := sha256.Sum256(a.Bytes)
	return hex.EncodeToString(sum[:])
}

type ArtifactProvider interface {
	Resolve(context.Context) (*Artifact, error)
}

type NoopArtifactProvider struct{}

func (NoopArtifactProvider) Resolve(context.Context) (*Artifact, error) {
	return nil, ErrEmbeddedArtifactUnavailable
}

type DefaultArtifactProvider struct{}

func (DefaultArtifactProvider) Resolve(context.Context) (*Artifact, error) {
	path := strings.TrimSpace(os.Getenv("CLIPROXY_DATA_PLANE_BINARY_PATH"))
	if path != "" {
		data, err := os.ReadFile(path)
		if err != nil {
			return nil, fmt.Errorf("read embedded data plane binary from %s: %w", path, err)
		}
		info, err := os.Stat(path)
		if err != nil {
			return nil, fmt.Errorf("stat embedded data plane binary from %s: %w", path, err)
		}
		return &Artifact{
			FileName: filepath.Base(path),
			Version:  fmt.Sprintf("file:%d:%d", info.Size(), info.ModTime().UTC().Unix()),
			Bytes:    data,
		}, nil
	}
	if artifact := bundledReleaseArtifact(); artifact != nil {
		return artifact, nil
	}
	return nil, ErrEmbeddedArtifactUnavailable
}

func ResolveStateDir(override string) (string, error) {
	if trimmed := strings.TrimSpace(override); trimmed != "" {
		return filepath.Abs(trimmed)
	}

	if base := strings.TrimSpace(util.WritablePath()); base != "" {
		return filepath.Join(base, "cliproxy", "data-plane"), nil
	}

	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("resolve user home for embedded data plane: %w", err)
	}

	switch runtime.GOOS {
	case "darwin":
		return filepath.Join(homeDir, "Library", "Application Support", "cliproxy", "data-plane"), nil
	case "windows":
		if localAppData := strings.TrimSpace(os.Getenv("LocalAppData")); localAppData != "" {
			return filepath.Join(localAppData, "cliproxy", "data-plane"), nil
		}
		return filepath.Join(homeDir, "AppData", "Local", "cliproxy", "data-plane"), nil
	default:
		if xdgStateHome := strings.TrimSpace(os.Getenv("XDG_STATE_HOME")); xdgStateHome != "" {
			return filepath.Join(xdgStateHome, "cliproxy", "data-plane"), nil
		}
		return filepath.Join(homeDir, ".local", "state", "cliproxy", "data-plane"), nil
	}
}

func MaterializeArtifact(stateDir string, artifact *Artifact) (path string, reused bool, err error) {
	if artifact == nil {
		return "", false, fmt.Errorf("materialize embedded data plane artifact: artifact is nil")
	}
	fileName := strings.TrimSpace(artifact.FileName)
	if fileName == "" {
		return "", false, fmt.Errorf("materialize embedded data plane artifact: file name is empty")
	}
	if len(artifact.Bytes) == 0 {
		return "", false, fmt.Errorf("materialize embedded data plane artifact: artifact bytes are empty")
	}

	resolvedDir, err := ResolveStateDir(stateDir)
	if err != nil {
		return "", false, err
	}
	if err := os.MkdirAll(resolvedDir, 0o755); err != nil {
		return "", false, fmt.Errorf("create embedded data plane state dir %s: %w", resolvedDir, err)
	}

	binaryPath := filepath.Join(resolvedDir, fileName)
	versionPath := binaryPath + ".version"
	shaPath := binaryPath + ".sha256"
	targetSHA := artifact.FileSHA256()

	if existingVersion, errVersion := os.ReadFile(versionPath); errVersion == nil {
		if existingSHA, errSHA := os.ReadFile(shaPath); errSHA == nil {
			if strings.TrimSpace(string(existingVersion)) == strings.TrimSpace(artifact.Version) &&
				strings.TrimSpace(string(existingSHA)) == targetSHA {
				if _, errStat := os.Stat(binaryPath); errStat == nil {
					return binaryPath, true, nil
				}
			}
		}
	}

	tempFile, err := os.CreateTemp(resolvedDir, fileName+".tmp-*")
	if err != nil {
		return "", false, fmt.Errorf("create embedded data plane temp binary: %w", err)
	}
	tempPath := tempFile.Name()
	if _, err := tempFile.Write(artifact.Bytes); err != nil {
		_ = tempFile.Close()
		_ = os.Remove(tempPath)
		return "", false, fmt.Errorf("write embedded data plane binary: %w", err)
	}
	if err := tempFile.Close(); err != nil {
		_ = os.Remove(tempPath)
		return "", false, fmt.Errorf("close embedded data plane temp binary: %w", err)
	}
	if runtime.GOOS != "windows" {
		if err := os.Chmod(tempPath, 0o755); err != nil {
			_ = os.Remove(tempPath)
			return "", false, fmt.Errorf("chmod embedded data plane binary: %w", err)
		}
	}
	if err := os.Rename(tempPath, binaryPath); err != nil {
		_ = os.Remove(tempPath)
		return "", false, fmt.Errorf("activate embedded data plane binary: %w", err)
	}
	if err := os.WriteFile(versionPath, []byte(strings.TrimSpace(artifact.Version)+"\n"), 0o644); err != nil {
		return "", false, fmt.Errorf("write embedded data plane version file: %w", err)
	}
	if err := os.WriteFile(shaPath, []byte(targetSHA+"\n"), 0o644); err != nil {
		return "", false, fmt.Errorf("write embedded data plane checksum file: %w", err)
	}
	return binaryPath, false, nil
}
