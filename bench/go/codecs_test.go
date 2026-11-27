package bench

import (
	"crypto/sha256"
	"encoding/hex"
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
