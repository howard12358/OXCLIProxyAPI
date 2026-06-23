package main

import (
	"crypto/sha256"
	"encoding/hex"
	"flag"
	"fmt"
	"os"
	"path/filepath"
)

func main() {
	var (
		sourcePath string
		fileName   string
		version    string
		outputDir  string
	)
	flag.StringVar(&sourcePath, "source", "", "Path to the compiled cliproxy-data-plane binary")
	flag.StringVar(&fileName, "file-name", "", "Embedded binary file name exposed to Go runtime")
	flag.StringVar(&version, "version", "", "Embedded artifact version label")
	flag.StringVar(&outputDir, "output-dir", "internal/dataplane/embedded", "Output directory for generated embed files")
	flag.Parse()

	if sourcePath == "" {
		exitf("missing --source")
	}
	if fileName == "" {
		fileName = filepath.Base(sourcePath)
	}
	if version == "" {
		exitf("missing --version")
	}

	data, err := os.ReadFile(sourcePath)
	if err != nil {
		exitf("read source binary: %v", err)
	}
	sum := sha256.Sum256(data)
	shaValue := hex.EncodeToString(sum[:])

	if err := os.MkdirAll(outputDir, 0o755); err != nil {
		exitf("create output dir: %v", err)
	}

	binPath := filepath.Join(outputDir, "release_artifact.bin")
	goPath := filepath.Join(outputDir, "release_artifact_generated.go")

	if err := os.WriteFile(binPath, data, 0o644); err != nil {
		exitf("write embedded artifact binary: %v", err)
	}
	if err := os.WriteFile(goPath, []byte(generatedSource(fileName, version, shaValue)), 0o644); err != nil {
		exitf("write embedded artifact Go source: %v", err)
	}
}

func generatedSource(fileName, version, shaValue string) string {
	return fmt.Sprintf(`//go:build release_embedded_artifact

package embedded

import _ "embed"

//go:embed release_artifact.bin
var bundledReleaseArtifactBytes []byte

func bundledReleaseArtifact() *Artifact {
	return &Artifact{
		FileName: %q,
		Version:  %q,
		SHA256:   %q,
		Bytes:    bundledReleaseArtifactBytes,
	}
}
`, fileName, version, shaValue)
}

func exitf(format string, args ...any) {
	_, _ = fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}
