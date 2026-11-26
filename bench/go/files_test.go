package bench

import (
	"os"
	"path/filepath"
	"testing"
)

// A file of about 1 MiB read whole as text, from the page cache once the
// setup has written it: an open, a read, a close and a copy into a string,
// where Tondo also checks that it is UTF-8.
func BenchmarkFiles(b *testing.B) {
	text := megabyte(true)
	file := filepath.Join(b.TempDir(), "other.txt")
	if err := os.WriteFile(file, []byte(text), 0o644); err != nil {
		b.Fatal(err)
	}
	b.Run("readText", func(b *testing.B) {
		for b.Loop() {
			data, err := os.ReadFile(file)
			if err != nil {
				b.Fatal(err)
			}
			sinkString = string(data)
		}
		if len(sinkString) != 1_048_506 || sinkString != text {
			b.Fatalf("readText gave %d bytes", len(sinkString))
		}
	})
}
