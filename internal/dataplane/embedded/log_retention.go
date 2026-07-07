package embedded

import (
	"context"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	log "github.com/sirupsen/logrus"
	"gopkg.in/natefinch/lumberjack.v2"
)

const (
	embeddedLogSubdir          = "data-plane"
	embeddedLogCleanerInterval = time.Minute
	embeddedLogMaxSizeMB       = 20
	embeddedLogMaxBackups      = 5
	embeddedLogMaxTotalSizeMB  = 256
)

func defaultLogWriterFactory(path string) (io.WriteCloser, error) {
	return &lumberjack.Logger{
		Filename:   path,
		MaxSize:    embeddedLogMaxSizeMB,
		MaxBackups: embeddedLogMaxBackups,
		MaxAge:     0,
		Compress:   false,
	}, nil
}

type logCleaner struct {
	cancel context.CancelFunc
}

func startEmbeddedLogCleaner(logDir string) *logCleaner {
	maxBytes := int64(embeddedLogMaxTotalSizeMB) * 1024 * 1024
	if maxBytes <= 0 {
		return nil
	}
	dir := strings.TrimSpace(logDir)
	if dir == "" {
		return nil
	}
	ctx, cancel := context.WithCancel(context.Background())
	cleaner := &logCleaner{cancel: cancel}
	go cleaner.run(ctx, filepath.Clean(dir), maxBytes)
	return cleaner
}

func (c *logCleaner) Stop() {
	if c == nil || c.cancel == nil {
		return
	}
	c.cancel()
}

func (c *logCleaner) run(ctx context.Context, logDir string, maxBytes int64) {
	ticker := time.NewTicker(embeddedLogCleanerInterval)
	defer ticker.Stop()

	cleanOnce := func() {
		deleted, err := enforceEmbeddedLogDirSizeLimit(logDir, maxBytes)
		if err != nil {
			log.WithError(err).Warn("embedded data plane: failed to enforce log directory size limit")
			return
		}
		if deleted > 0 {
			log.WithField("deleted", deleted).Debug("embedded data plane: removed old rotated log files")
		}
	}

	cleanOnce()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			cleanOnce()
		}
	}
}

func enforceEmbeddedLogDirSizeLimit(logDir string, maxBytes int64) (int, error) {
	if maxBytes <= 0 {
		return 0, nil
	}
	dir := strings.TrimSpace(logDir)
	if dir == "" {
		return 0, nil
	}
	dir = filepath.Clean(dir)

	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return 0, nil
		}
		return 0, err
	}

	type logFile struct {
		path    string
		size    int64
		modTime time.Time
	}

	var (
		files []logFile
		total int64
	)
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		name := strings.TrimSpace(entry.Name())
		if !isEmbeddedLogFileName(name) {
			continue
		}
		info, errInfo := entry.Info()
		if errInfo != nil || !info.Mode().IsRegular() {
			continue
		}
		path := filepath.Join(dir, name)
		files = append(files, logFile{path: path, size: info.Size(), modTime: info.ModTime()})
		total += info.Size()
	}
	if total <= maxBytes {
		return 0, nil
	}

	sort.Slice(files, func(i, j int) bool { return files[i].modTime.Before(files[j].modTime) })

	deleted := 0
	for _, file := range files {
		if total <= maxBytes {
			break
		}
		if isEmbeddedActiveLogFile(filepath.Base(file.path)) {
			continue
		}
		if errRemove := os.Remove(file.path); errRemove != nil {
			log.WithError(errRemove).Warnf("embedded data plane: failed to remove old log file: %s", filepath.Base(file.path))
			continue
		}
		total -= file.size
		deleted++
	}
	return deleted, nil
}

func isEmbeddedLogFileName(name string) bool {
	lower := strings.ToLower(strings.TrimSpace(name))
	return strings.HasSuffix(lower, ".log") || strings.HasSuffix(lower, ".log.gz")
}

func isEmbeddedActiveLogFile(name string) bool {
	switch strings.TrimSpace(name) {
	case "stdout.log", "stderr.log":
		return true
	default:
		return false
	}
}
