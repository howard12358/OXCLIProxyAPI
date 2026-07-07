package embedded

import (
	"bytes"
	"testing"
)

func TestPrefixedLineWriterPrefixesCompleteAndTrailingLines(t *testing.T) {
	t.Parallel()

	var out bytes.Buffer
	writer := newPrefixedLineWriter("[rs-stdout] ", &out)

	if _, err := writer.Write([]byte("first line\nsecond")); err != nil {
		t.Fatalf("Write() error = %v", err)
	}
	if _, err := writer.Write([]byte(" line\nthird")); err != nil {
		t.Fatalf("Write() second error = %v", err)
	}
	if err := writer.Close(); err != nil {
		t.Fatalf("Close() error = %v", err)
	}

	want := "[rs-stdout] first line\n[rs-stdout] second line\n[rs-stdout] third"
	if got := out.String(); got != want {
		t.Fatalf("output = %q, want %q", got, want)
	}
}
