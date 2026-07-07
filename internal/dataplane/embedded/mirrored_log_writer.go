package embedded

import (
	"bytes"
	"io"
	"os"
	"sync"

	"gopkg.in/natefinch/lumberjack.v2"
)

type mirroredLogWriter struct {
	file   io.WriteCloser
	mirror *prefixedLineWriter
}

func newMirroredLogWriter(path string, prefix string, mirrorTarget io.Writer) (io.WriteCloser, error) {
	fileWriter := &lumberjack.Logger{
		Filename:   path,
		MaxSize:    embeddedLogMaxSizeMB,
		MaxBackups: embeddedLogMaxBackups,
		MaxAge:     0,
		Compress:   false,
	}
	return &mirroredLogWriter{
		file:   fileWriter,
		mirror: newPrefixedLineWriter(prefix, mirrorTarget),
	}, nil
}

func (w *mirroredLogWriter) Write(p []byte) (int, error) {
	if w == nil {
		return 0, nil
	}
	n, err := w.file.Write(p)
	if err != nil {
		return n, err
	}
	if _, errMirror := w.mirror.Write(p); errMirror != nil {
		return n, errMirror
	}
	return n, nil
}

func (w *mirroredLogWriter) Close() error {
	if w == nil {
		return nil
	}
	if err := w.mirror.Close(); err != nil {
		_ = w.file.Close()
		return err
	}
	return w.file.Close()
}

type prefixedLineWriter struct {
	mu     sync.Mutex
	prefix []byte
	target io.Writer
	buffer bytes.Buffer
}

func newPrefixedLineWriter(prefix string, target io.Writer) *prefixedLineWriter {
	return &prefixedLineWriter{
		prefix: []byte(prefix),
		target: target,
	}
}

func (w *prefixedLineWriter) Write(p []byte) (int, error) {
	if w == nil || len(p) == 0 {
		return len(p), nil
	}
	w.mu.Lock()
	defer w.mu.Unlock()

	w.buffer.Write(p)
	for {
		raw := w.buffer.Bytes()
		idx := bytes.IndexByte(raw, '\n')
		if idx < 0 {
			return len(p), nil
		}
		line := append([]byte(nil), raw[:idx+1]...)
		w.buffer.Next(idx + 1)
		if errWrite := w.writeLine(line); errWrite != nil {
			return 0, errWrite
		}
	}
}

func (w *prefixedLineWriter) Close() error {
	if w == nil {
		return nil
	}
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.buffer.Len() == 0 {
		return nil
	}
	line := append([]byte(nil), w.buffer.Bytes()...)
	w.buffer.Reset()
	return w.writeLine(line)
}

func (w *prefixedLineWriter) writeLine(line []byte) error {
	if len(line) == 0 {
		return nil
	}
	payload := make([]byte, 0, len(w.prefix)+len(line))
	payload = append(payload, w.prefix...)
	payload = append(payload, line...)
	_, err := w.target.Write(payload)
	return err
}

func defaultStdoutWriterFactory(path string) (io.WriteCloser, error) {
	return newMirroredLogWriter(path, "[rs-stdout] ", os.Stdout)
}

func defaultStderrWriterFactory(path string) (io.WriteCloser, error) {
	return newMirroredLogWriter(path, "[rs-stderr] ", os.Stderr)
}
