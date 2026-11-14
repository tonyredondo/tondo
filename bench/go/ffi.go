//go:build cgo

// The Go side of `ffi`: Go called from C, as Tondo is there, through
// callers.c; cgo allows no C in a test file.
package bench

/*
#cgo LDFLAGS: -lpthread
typedef long (*called)(long, long);
long calls(called function, long times);
long calls_from_threads(called function, long threads, long times);
long goAdd(long, long);
long goWaits(long, long);
*/
import "C"

//export goAdd
func goAdd(total, i C.long) C.long {
	return total + i
}

//export goWaits
func goWaits(total, i C.long) C.long {
	done := make(chan C.long)
	go func() { done <- 1 }()
	return total + i + <-done
}

func callback(times int) int {
	return int(C.calls(C.called(C.goAdd), C.long(times)))
}

func fromThreads(threads, times int, waits bool) int {
	function := C.called(C.goAdd)
	if waits {
		function = C.called(C.goWaits)
	}
	return int(C.calls_from_threads(function, C.long(threads), C.long(times)))
}
