package bench

import (
	"bufio"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A file of about 1 MiB read whole as text, from the page cache once the
// setup has written it: an open, a read, a close and a copy into a string,
// where Tondo also checks that it is UTF-8. Then four mebibytes of lines
// read one at a time and written one at a time, through each side's
// buffer: its refills and its calls to the system.
func BenchmarkFiles(b *testing.B) {
	dir := b.TempDir()
	text := megabyte(true)
	file := filepath.Join(dir, "other.txt")
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
	line := strings.TrimSpace(asciiWords)
	lines := strings.Repeat(line+"\n", 95_325)
	linesFile := filepath.Join(dir, "lines.txt")
	if err := os.WriteFile(linesFile, []byte(lines), 0o644); err != nil {
		b.Fatal(err)
	}
	b.Run("readLines 4MiB", func(b *testing.B) {
		for b.Loop() {
			sinkInt = readLines(b, linesFile)
		}
		if sinkInt != 4_098_975 {
			b.Fatalf("readLines 4MiB gave %d bytes", sinkInt)
		}
	})
	written := filepath.Join(dir, "written.txt")
	b.Run("writeLines 4MiB", func(b *testing.B) {
		for b.Loop() {
			writeLines(b, written, line, 95_325)
		}
		data, err := os.ReadFile(written)
		if err != nil || string(data) != lines || len(lines) != 4_194_300 {
			b.Fatalf("writeLines 4MiB wrote %d bytes: %v", len(data), err)
		}
	})
}

// readLines gives the bytes of the file's lines without their ends, read
// one at a time.
func readLines(b *testing.B, path string) int {
	file, err := os.Open(path)
	if err != nil {
		b.Fatal(err)
	}
	defer file.Close()
	scanner := bufio.NewScanner(file)
	total := 0
	for scanner.Scan() {
		total += len(scanner.Text())
	}
	if err := scanner.Err(); err != nil {
		b.Fatal(err)
	}
	return total
}

// writeLines writes the line count times, each with its "\n".
func writeLines(b *testing.B, path, line string, count int) {
	file, err := os.Create(path)
	if err != nil {
		b.Fatal(err)
	}
	writer := bufio.NewWriter(file)
	for i := 0; i < count; i++ {
		writer.WriteString(line)
		writer.WriteByte('\n')
	}
	if err := writer.Flush(); err != nil {
		b.Fatal(err)
	}
	if err := file.Close(); err != nil {
		b.Fatal(err)
	}
}
