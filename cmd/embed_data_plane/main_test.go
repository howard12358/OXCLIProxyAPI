package main

import (
	"strings"
	"testing"
)

func TestGeneratedSourceIncludesExpectedMetadata(t *testing.T) {
	t.Parallel()

	source := generatedSource("cliproxy-data-plane.exe", "1.2.3", "abc123")
	for _, needle := range []string{
		"//go:build release_embedded_artifact",
		"//go:embed release_artifact.bin",
		`FileName: "cliproxy-data-plane.exe"`,
		`Version:  "1.2.3"`,
		`SHA256:   "abc123"`,
	} {
		if !strings.Contains(source, needle) {
			t.Fatalf("generated source missing %q:\n%s", needle, source)
		}
	}
}
