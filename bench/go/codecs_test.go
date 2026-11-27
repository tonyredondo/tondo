package bench

import (
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"io"
	"testing"
)

// A mebibyte hashed with SHA-256: whether each side takes the processor's
// instructions for it.
func BenchmarkHash(b *testing.B) {
	data := []byte(megabyte(true))
	b.Run("sha256 1MiB", func(b *testing.B) {
		var digest [32]byte
		for b.Loop() {
			digest = sha256.Sum256(data)
		}
		if hex.EncodeToString(digest[:]) != "0ea51d8089aad80e26dcd0889b7db4cee183e7b84bad16e5dc91ce3e584d3efc" {
			b.Fatalf("sha256 1MiB gave %x", digest)
		}
	})
}

// gzipped compresses the data at level 6.
func gzipped(b *testing.B, data []byte) []byte {
	var out bytes.Buffer
	writer, err := gzip.NewWriterLevel(&out, 6)
	if err != nil {
		b.Fatal(err)
	}
	writer.Write(data)
	if err := writer.Close(); err != nil {
		b.Fatal(err)
	}
	return out.Bytes()
}

// gunzipped decompresses the data whole.
func gunzipped(b *testing.B, data []byte) []byte {
	reader, err := gzip.NewReader(bytes.NewReader(data))
	if err != nil {
		b.Fatal(err)
	}
	unpacked, err := io.ReadAll(reader)
	if err != nil {
		b.Fatal(err)
	}
	return unpacked
}

// A mebibyte of JSON, which varies as data does, compressed at gzip's level
// 6 and decompressed again, each side decompressing what it compressed.
func BenchmarkGzip(b *testing.B) {
	data, err := json.Marshal(sales(12_548))
	if err != nil {
		b.Fatal(err)
	}
	b.Run("compress 1MiB", func(b *testing.B) {
		var packed []byte
		for b.Loop() {
			packed = gzipped(b, data)
			sinkAny = packed
		}
		if !bytes.Equal(gunzipped(b, packed), data) || len(packed) >= len(data)/4 {
			b.Fatalf("compress 1MiB gave %d bytes that are not the data", len(packed))
		}
	})
	packed := gzipped(b, data)
	b.Run("decompress 1MiB", func(b *testing.B) {
		var unpacked []byte
		for b.Loop() {
			unpacked = gunzipped(b, packed)
			sinkAny = unpacked
		}
		if !bytes.Equal(unpacked, data) {
			b.Fatalf("decompress 1MiB gave %d bytes", len(unpacked))
		}
	})
}

// A mebibyte encoded in base64 and decoded again.
func BenchmarkEncoding(b *testing.B) {
	data := []byte(megabyte(true))
	b.Run("base64 encode 1MiB", func(b *testing.B) {
		for b.Loop() {
			sinkString = base64.StdEncoding.EncodeToString(data)
		}
		if len(sinkString) != 1_398_008 || fnv64a(sinkString) != 9913155958480842097 {
			b.Fatalf("base64 encode 1MiB gave %d bytes", len(sinkString))
		}
	})
	encoded := base64.StdEncoding.EncodeToString(data)
	b.Run("base64 decode 1MiB", func(b *testing.B) {
		var decoded []byte
		for b.Loop() {
			var err error
			decoded, err = base64.StdEncoding.DecodeString(encoded)
			if err != nil {
				b.Fatal(err)
			}
			sinkAny = decoded
		}
		if !bytes.Equal(decoded, data) {
			b.Fatalf("base64 decode 1MiB gave %d bytes", len(decoded))
		}
	})
}
