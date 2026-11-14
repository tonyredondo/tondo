//go:build cgo

package bench

import "testing"

func BenchmarkFfi(b *testing.B) {
	b.Run("callback_1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = callback(1000)
		}
	})
	b.Run("from_a_C_thread_1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = fromThreads(1, 1000, false)
		}
	})
	b.Run("from_C_threads_8x1000", func(b *testing.B) {
		for b.Loop() {
			sinkInt = fromThreads(8, 1000, false)
		}
	})
	b.Run("waits_from_a_C_thread_100", func(b *testing.B) {
		for b.Loop() {
			sinkInt = fromThreads(1, 100, true)
		}
	})
}
