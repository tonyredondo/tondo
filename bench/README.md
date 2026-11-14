# Tondo against Go

The same work in Tondo and in Go, measured side by side: `tondo/` is a
project whose benchmarks are its tests (`testing.bench`), `go/` a module
whose benchmarks have the same names (`BenchmarkCalls/fib_25` is the
benchmark `fib 25` of the test `calls`), each kept from the optimizer as
the other is. `compare.to`, a Tondo program, runs both and writes the table:

```sh
tondo run bench/compare.to -- --time 1 --output bench/results.md
```

Each side measures a run as the median of its samples; Go's `-benchmem`
and Tondo's `--bench` count allocations the same way, each in its own
allocator. The goal is every row at or under 1.0×.

Where `perf` counts on the machine, the table also has the instructions
and cycles of a run on each side, outside the kernel and on every thread:
Tondo's from `--bench`, Go's from its test program run under `perf stat`
so many times and twice as many, whose difference leaves out the start of
the program and Go's first run. The instructions do not move with where
the code lies, as the time does: a row slower in time with fewer
instructions than Go is slower for its cache misses, branches or layout,
and one with more instructions does more work.

The rows of `http/` and `net/` run a server and its clients in one
process, so a row mixes the two sides of each request, which a profile
tells apart: HTTP/1.1 in the clear and HTTP/2 over TLS, as Tondo's client
and server negotiate them, with the certificate in `tondo/src/tls`, made
for these benchmarks alone.

The rows of `ffi/` call each side from C: a loop of C that calls a function
given to it, from the benchmark's own thread, which in Tondo is a task's,
and from threads that C starts and the runtime has never seen, and a call
that waits once, for a task or a goroutine it starts. Each side compiles
the same `callers.c` with the system's C compiler, Go's through cgo; without
one, Tondo's test skips them and Go builds without them.

The tables were measured on an AMD Ryzen 9 5950X, Linux, with Go 1.27.
Now, on 2026-11-13 at b42ce555, each side for its default level of
processor: Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| tasks/spawn 1000 | 179.29 µs | 171.38 µs | 1.05× ⚠ | 1953 | 2002 | 2.27M | 2.35M | 1.41M | 1.79M |
| net/tls echo 1x1000 | 12.66 ms | 12.24 ms | 1.03× ⚠ | 7000 | 3745 | 18.66M | 14.19M | 14.42M | 11.56M |
| floats/sqrt 100000 | 180.09 µs | 176.01 µs | 1.02× ⚠ | 0 | 0 | 725.0k | 700.0k | 897.1k | 858.4k |
| runtime/mutex 4x1000 | 21.77 µs | 21.44 µs | 1.01× ⚠ | 6 | 6 | 409.0k | 118.6k | 99.8k | 144.0k |
| strings/split 100 | 764.0 ns | 753.2 ns | 1.01× ⚠ | 1 | 1 | 16.0k | 13.7k | 3.6k | 3.9k |
| net/echo 1x1000 | 11.38 ms | 11.38 ms | 1.00× ⚠ | 3000 | 3744 | 10.60M | 7.12M | 9.26M | 7.42M |
| calls/closure 1000 | 204.9 ns | 209.3 ns | 0.98× | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| floats/mandelbrot 64 | 137.64 µs | 140.64 µs | 0.98× | 0 | 0 | 1.49M | 1.57M | 682.1k | 687.1k |
| strings/builder 100 | 765.1 ns | 789.1 ns | 0.97× | 6 | 8 | 16.1k | 13.4k | 3.7k | 4.2k |
| json/decode struct | 1.23 µs | 1.29 µs | 0.96× | 2 | 4 | 18.0k | 21.0k | 5.9k | 6.3k |
| arrays/index sum 10000 | 2.09 µs | 2.19 µs | 0.96× | 0 | 0 | 45.0k | 40.0k | 10.1k | 10.6k |
| loops/collatz 10000 | 737.82 µs | 843.77 µs | 0.87× | 0 | 0 | 15.37M | 8.86M | 3.60M | 4.18M |
| runtime/select 1000 | 125.54 µs | 144.39 µs | 0.87× | 4 | 4 | 1.34M | 2.12M | 550.9k | 738.6k |
| loops/sum 1000000 | 612.78 µs | 724.78 µs | 0.85× | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.51M |
| json/encode 100 structs | 25.76 µs | 30.47 µs | 0.85× | 2 | 4 | 406.6k | 439.9k | 123.0k | 144.9k |
| runtime/join 1 | 262.6 ns | 316.5 ns | 0.83× | 1 | 2 | 2.8k | 3.2k | 1.2k | 1.4k |
| strings/contains 1000 | 13.0 ns | 15.8 ns | 0.82× | 0 | 0 | 332 | 394 | 62 | 76 |
| http/hello 1x1000 | 33.00 ms | 40.31 ms | 0.82× | 67000 | 69199 | 71.39M | 144.74M | 83.08M | 139.02M |
| calls/fib 25 | 232.47 µs | 289.79 µs | 0.80× | 0 | 0 | 4.81M | 3.52M | 1.13M | 1.37M |
| strings/int to text 1000 | 7.99 µs | 10.97 µs | 0.73× | 0 | 900 | 163.6k | 211.3k | 38.2k | 52.3k |
| strings/interpolate | 23.6 ns | 32.8 ns | 0.72× | 1 | 1 | 501 | 662 | 113 | 159 |
| http/tls 64x100 | 62.66 ms | 87.47 ms | 0.72× | 494266 | 460518 | 612.19M | 1.69G | 919.33M | 1.45G |
| tasks/channel 10000 | 194.23 µs | 275.52 µs | 0.70× | 2 | 2 | 3.17M | 4.41M | 918.7k | 1.30M |
| json/parse object | 2.33 µs | 3.57 µs | 0.65× | 17 | 36 | 34.9k | 47.5k | 11.2k | 16.8k |
| maps/lookup 10000 ints | 54.51 µs | 86.58 µs | 0.63× | 0 | 0 | 970.1k | 1.06M | 258.8k | 367.9k |
| arrays/map filter sum 10000 | 16.79 µs | 31.18 µs | 0.54× | 12 | 16 | 374.0k | 481.9k | 80.6k | 194.4k |
| json/parse 1000 numbers | 74.78 µs | 138.98 µs | 0.54× | 909 | 2015 | 1.38M | 2.12M | 355.8k | 671.2k |
| structs/users 1000 | 19.96 µs | 37.70 µs | 0.53× | 1001 | 1910 | 412.9k | 663.9k | 95.6k | 187.7k |
| http/json 64x100 | 52.53 ms | 103.42 ms | 0.51× | 672467 | 713393 | 765.29M | 3.79G | 2.29G | 6.43G |
| maps/insert 1000 strings | 46.35 µs | 97.60 µs | 0.47× | 1030 | 1920 | 684.1k | 1.30M | 221.4k | 466.9k |
| maps/insert 10000 ints | 135.54 µs | 492.18 µs | 0.28× | 42 | 79 | 2.33M | 5.35M | 650.1k | 2.28M |
| regex/match 1000 | 38.15 µs | 148.93 µs | 0.26× | 0 | 0 | 573.3k | 2.99M | 184.3k | 713.5k |
| structs/points 1000 | 1.57 µs | 6.60 µs | 0.24× | 1 | 12 | 34.6k | 80.6k | 7.6k | 51.9k |
| http/hello 64x100 | 20.99 ms | 104.17 ms | 0.20× | 429217 | 454052 | 456.02M | 3.31G | 1.17G | 7.93G |
| arrays/sort 10000 | 66.64 µs | 357.52 µs | 0.19× | 1 | 1 | 1.37M | 2.41M | 321.1k | 1.68M |
| arrays/push 1000 | 511.0 ns | 3.08 µs | 0.17× | 1 | 12 | 13.0k | 42.0k | 2.4k | 23.2k |

6 of 36 benchmarks are slower than Go; 9 run more instructions than Go.

`tasks/spawn` moves with which cores a run's threads land on: the
5950X's two sets of eight cores share no cache, and Tondo's workers keep to
those that share one (docs/decisions.md). `floats/sqrt`, `runtime/mutex`,
`strings/split` and `net/echo` are within 2% of Go, as much as a row moves
from one run to the next, and the last three were under Go's time in the
measurement before this one; `runtime/mutex` and `strings/split` take
fewer cycles than Go's. `loops/collatz` runs 1.7 times Go's instructions,
and its time is where its code lies (below). `net/tls echo` makes about three more allocations a round trip
than Go, and `http/tls` a few more a request: rustls 0.23 makes a buffer for
each record it encrypts and copies each one it reads into another, in its
buffered API and its unbuffered one alike.

Both for x86-64-v3, `--cpu x86-64-v3` and `GOAMD64=v3`, with
`compare.to --cpu x86-64-v3`, the same day:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| floats/mandelbrot 64 | 136.75 µs | 127.84 µs | 1.07× ⚠ | 0 | 0 | 1.20M | 1.42M | 675.8k | 623.3k |
| loops/collatz 10000 | 901.68 µs | 843.98 µs | 1.07× ⚠ | 0 | 0 | 15.37M | 8.86M | 4.40M | 4.14M |
| tasks/spawn 1000 | 180.11 µs | 172.21 µs | 1.05× ⚠ | 1953 | 2002 | 2.27M | 2.32M | 1.41M | 1.79M |
| net/tls echo 1x1000 | 12.73 ms | 12.18 ms | 1.04× ⚠ | 7000 | 3744 | 18.66M | 14.21M | 14.35M | 11.28M |
| runtime/mutex 4x1000 | 21.62 µs | 20.96 µs | 1.03× ⚠ | 6 | 6 | 409.0k | 119.1k | 100.0k | 143.7k |
| floats/sqrt 100000 | 178.23 µs | 176.78 µs | 1.01× ⚠ | 0 | 0 | 725.0k | 700.5k | 891.3k | 883.7k |
| net/echo 1x1000 | 11.23 ms | 11.28 ms | 1.00× | 3000 | 3744 | 10.60M | 7.16M | 8.85M | 7.39M |
| strings/split 100 | 758.8 ns | 766.1 ns | 0.99× | 1 | 1 | 16.0k | 13.6k | 3.6k | 3.7k |
| calls/closure 1000 | 204.5 ns | 208.4 ns | 0.98× | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| strings/builder 100 | 761.6 ns | 786.4 ns | 0.97× | 6 | 8 | 16.1k | 13.4k | 3.6k | 3.9k |
| arrays/index sum 10000 | 2.07 µs | 2.16 µs | 0.96× | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.6k |
| json/decode struct | 1.19 µs | 1.30 µs | 0.92× | 2 | 4 | 18.1k | 21.0k | 5.7k | 6.2k |
| runtime/select 1000 | 126.93 µs | 144.39 µs | 0.88× | 4 | 4 | 1.34M | 2.10M | 556.5k | 736.1k |
| loops/sum 1000000 | 610.54 µs | 725.39 µs | 0.84× | 0 | 0 | 8.25M | 15.00M | 3.00M | 3.56M |
| runtime/join 1 | 261.5 ns | 311.0 ns | 0.84× | 1 | 2 | 2.8k | 3.3k | 1.2k | 1.4k |
| http/hello 1x1000 | 33.49 ms | 40.38 ms | 0.83× | 67000 | 69185 | 71.37M | 145.33M | 85.22M | 132.03M |
| strings/contains 1000 | 13.0 ns | 15.8 ns | 0.82× | 0 | 0 | 332 | 394 | 62 | 76 |
| json/encode 100 structs | 23.42 µs | 29.57 µs | 0.79× | 2 | 4 | 406.6k | 440.9k | 112.5k | 141.7k |
| maps/lookup 10000 ints | 54.71 µs | 70.20 µs | 0.78× | 0 | 0 | 970.1k | 1.01M | 260.9k | 332.4k |
| http/tls 64x100 | 64.66 ms | 85.81 ms | 0.75× | 494408 | 460480 | 612.26M | 1.72G | 930.90M | 1.67G |
| calls/fib 25 | 217.24 µs | 289.77 µs | 0.75× | 0 | 0 | 4.81M | 3.52M | 1.05M | 1.42M |
| strings/interpolate | 23.6 ns | 32.7 ns | 0.72× | 1 | 1 | 501 | 664 | 113 | 154 |
| tasks/channel 10000 | 191.63 µs | 285.29 µs | 0.67× | 2 | 2 | 3.17M | 4.42M | 910.5k | 1.37M |
| strings/int to text 1000 | 7.96 µs | 11.88 µs | 0.67× | 0 | 900 | 163.6k | 211.2k | 38.3k | 55.9k |
| json/parse object | 2.27 µs | 3.52 µs | 0.64× | 17 | 36 | 34.9k | 48.2k | 10.9k | 17.4k |
| json/parse 1000 numbers | 74.10 µs | 136.48 µs | 0.54× | 909 | 2015 | 1.38M | 2.11M | 355.7k | 638.5k |
| arrays/map filter sum 10000 | 16.90 µs | 32.47 µs | 0.52× | 12 | 16 | 374.0k | 477.3k | 80.8k | 196.2k |
| structs/users 1000 | 19.51 µs | 39.24 µs | 0.50× | 1001 | 1910 | 409.6k | 665.6k | 93.3k | 194.3k |
| maps/insert 1000 strings | 47.89 µs | 97.28 µs | 0.49× | 1030 | 1920 | 680.1k | 1.30M | 230.0k | 462.9k |
| http/json 64x100 | 52.08 ms | 106.12 ms | 0.49× | 672515 | 715121 | 758.18M | 3.71G | 2.25G | 6.22G |
| maps/insert 10000 ints | 135.65 µs | 493.27 µs | 0.27× | 42 | 79 | 2.33M | 5.28M | 651.3k | 2.37M |
| regex/match 1000 | 38.27 µs | 153.81 µs | 0.25× | 0 | 0 | 573.3k | 2.99M | 183.8k | 755.1k |
| structs/points 1000 | 1.57 µs | 6.72 µs | 0.23× | 1 | 12 | 34.6k | 78.0k | 7.5k | 46.9k |
| http/hello 64x100 | 20.76 ms | 105.92 ms | 0.20× | 429298 | 454438 | 455.87M | 3.34G | 1.17G | 8.24G |
| arrays/sort 10000 | 66.87 µs | 361.04 µs | 0.19× | 1 | 1 | 1.37M | 2.41M | 321.4k | 1.71M |
| arrays/push 1000 | 510.8 ns | 3.20 µs | 0.16× | 1 | 12 | 13.0k | 39.8k | 2.4k | 21.9k |

6 of 36 benchmarks are slower than Go; 9 run more instructions than Go.

Go makes more of FMA in `floats/mandelbrot` at this level, and
`loops/collatz`, whose code is the same at both levels, is where its code
lies: from 736 µs to 871 µs over four orders of the functions at
`baseline`, and from 866 µs to 899 µs at `x86-64-v3`.

What the level changes of Tondo's own code, every benchmark at `baseline`
and at `x86-64-v3`, by the instructions and cycles of a run, the fewest
instructions at `x86-64-v3` first. The generator takes FMA, BMI, LZCNT and
the shorter encodings of AVX at `x86-64-v3`, and makes no vector of 256
bits, so only the floats' and the integers' loops run fewer instructions,
and the cycles move as where the code lies moves them:

| Benchmark | Ins/op baseline | Ins/op x86-64-v3 | Change | Cycles/op baseline | Cycles/op x86-64-v3 | Change |
|---|---:|---:|---:|---:|---:|---:|
| floats/mandelbrot 64 | 1.49M | 1.20M | -19.7% | 682.0k | 675.6k | -0.9% |
| loops/sum 1000000 | 9.25M | 8.25M | -10.8% | 3.00M | 3.01M | +0.3% |
| structs/users 1000 | 412.9k | 409.6k | -0.8% | 96.0k | 91.7k | -4.4% |
| maps/insert 1000 strings | 684.1k | 680.1k | -0.6% | 220.0k | 229.1k | +4.1% |
| json/decode struct | 18.1k | 18.0k | -0.2% | 5.8k | 5.7k | -2.0% |
| http/tls 64x100 | 613.66M | 612.90M | -0.1% | 944.14M | 938.19M | -0.6% |
| http/json 64x100 | 765.10M | 764.15M | -0.1% | 2.28G | 2.26G | -0.8% |
| net/echo 1x1000 | 10.60M | 10.60M | -0.0% | 8.79M | 8.87M | +0.9% |
| http/hello 1x1000 | 71.41M | 71.39M | -0.0% | 84.90M | 83.80M | -1.3% |
| net/tls echo 1x1000 | 18.66M | 18.65M | -0.0% | 14.62M | 14.79M | +1.2% |
| runtime/join 1 | 2.8k | 2.8k | -0.0% | 1.3k | 1.2k | -3.8% |
| http/hello 64x100 | 455.77M | 455.72M | -0.0% | 1.19G | 1.19G | +0.4% |
| runtime/mutex 4x1000 | 409.1k | 409.0k | -0.0% | 101.8k | 99.8k | -2.0% |
| tasks/channel 10000 | 3.17M | 3.17M | -0.0% | 926.0k | 909.5k | -1.8% |
| yaml/encode 100 structs | 1.55M | 1.55M | -0.0% | 417.2k | 434.5k | +4.1% |
| json/encode 100 structs | 406.6k | 406.6k | -0.0% | 121.2k | 112.6k | -7.1% |
| floats/sqrt 100000 | 725.0k | 725.0k | -0.0% | 895.6k | 890.3k | -0.6% |
| json/parse 1000 numbers | 1.38M | 1.38M | -0.0% | 356.1k | 360.8k | +1.3% |
| calls/fib 25 | 4.81M | 4.81M | -0.0% | 1.13M | 1.06M | -6.6% |
| arrays/map filter sum 10000 | 374.0k | 374.0k | -0.0% | 81.5k | 80.9k | -0.7% |
| arrays/index sum 10000 | 45.0k | 45.0k | +0.0% | 10.0k | 10.0k | -0.0% |
| calls/closure 1000 | 3.3k | 3.3k | +0.0% | 1.0k | 1.0k | +0.0% |
| strings/interpolate | 501 | 501 | +0.0% | 112 | 113 | +0.7% |
| strings/builder 100 | 16.1k | 16.1k | +0.0% | 3.7k | 3.7k | +0.7% |
| strings/int to text 1000 | 163.6k | 163.6k | +0.0% | 38.2k | 38.2k | -0.0% |
| strings/contains 1000 | 332 | 332 | +0.0% | 62 | 62 | +0.3% |
| regex/match 1000 | 573.3k | 573.3k | +0.0% | 184.5k | 183.8k | -0.3% |
| maps/lookup 10000 ints | 970.1k | 970.1k | +0.0% | 259.3k | 260.9k | +0.6% |
| arrays/sort 10000 | 1.37M | 1.37M | +0.0% | 321.0k | 319.1k | -0.6% |
| structs/points 1000 | 34.6k | 34.6k | +0.0% | 7.5k | 7.5k | -0.0% |
| arrays/push 1000 | 13.0k | 13.0k | +0.0% | 2.4k | 2.4k | -0.1% |
| loops/collatz 10000 | 15.37M | 15.37M | +0.0% | 3.60M | 4.40M | +22.2% |
| strings/split 100 | 16.0k | 16.0k | +0.0% | 3.6k | 3.6k | -0.3% |
| yaml/decode 100 structs | 3.69M | 3.69M | +0.0% | 1.45M | 1.47M | +1.4% |
| json/parse object | 34.9k | 34.9k | +0.0% | 11.1k | 11.0k | -0.9% |
| runtime/select 1000 | 1.34M | 1.34M | +0.0% | 553.0k | 555.5k | +0.5% |
| tasks/spawn 1000 | 2.27M | 2.27M | +0.1% | 1.42M | 1.42M | +0.3% |
| maps/insert 10000 ints | 2.33M | 2.33M | +0.2% | 649.3k | 648.7k | -0.1% |

On 2026-10-26, before any of the work they call for:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go |
|---|---:|---:|---:|---:|---:|
| strings/builder 100 | 36.29 µs | 772.7 ns | 46.96× ⚠ | 2425 | 8 |
| calls/closure 1000 | 4.98 µs | 206.9 ns | 24.06× ⚠ | 1 | 0 |
| arrays/sort 10000 | 3.26 ms | 356.78 µs | 9.14× ⚠ | 47934 | 1 |
| tasks/spawn 1000 | 1.20 ms | 172.21 µs | 6.95× ⚠ | 4010 | 2002 |
| arrays/map filter sum 10000 | 203.53 µs | 32.13 µs | 6.34× ⚠ | 28 | 16 |
| arrays/index sum 10000 | 10.17 µs | 2.18 µs | 4.67× ⚠ | 0 | 0 |
| floats/sqrt 100000 | 726.82 µs | 171.51 µs | 4.24× ⚠ | 0 | 0 |
| tasks/channel 10000 | 1.16 ms | 281.20 µs | 4.11× ⚠ | 208 | 2 |
| strings/split 100 | 2.92 µs | 758.4 ns | 3.86× ⚠ | 107 | 1 |
| strings/int to text 1000 | 42.91 µs | 11.47 µs | 3.74× ⚠ | 3000 | 900 |
| json/decode struct | 3.68 µs | 1.28 µs | 2.88× ⚠ | 67 | 4 |
| strings/interpolate | 78.1 ns | 33.1 ns | 2.36× ⚠ | 5 | 1 |
| strings/contains 1000 | 30.2 ns | 15.8 ns | 1.91× ⚠ | 0 | 0 |
| arrays/push 1000 | 5.57 µs | 3.16 µs | 1.76× ⚠ | 10 | 12 |
| structs/users 1000 | 66.56 µs | 38.29 µs | 1.74× ⚠ | 3010 | 1910 |
| loops/collatz 10000 | 1.28 ms | 835.02 µs | 1.53× ⚠ | 0 | 0 |
| loops/sum 1000000 | 1.06 ms | 718.64 µs | 1.48× ⚠ | 0 | 0 |
| json/parse 1000 numbers | 168.10 µs | 125.05 µs | 1.34× ⚠ | 4013 | 2015 |
| calls/fib 25 | 381.61 µs | 287.75 µs | 1.33× ⚠ | 0 | 0 |
| json/parse object | 4.49 µs | 3.56 µs | 1.26× ⚠ | 85 | 36 |
| structs/points 1000 | 7.43 µs | 6.72 µs | 1.11× ⚠ | 10 | 12 |
| json/encode 100 structs | 32.37 µs | 29.48 µs | 1.10× ⚠ | 114 | 4 |
| floats/mandelbrot 64 | 150.72 µs | 138.43 µs | 1.09× ⚠ | 0 | 0 |
| maps/insert 1000 strings | 81.91 µs | 99.58 µs | 0.82× | 3030 | 1920 |
| maps/lookup 10000 ints | 60.81 µs | 75.34 µs | 0.81× | 0 | 0 |
| maps/insert 10000 ints | 399.68 µs | 500.99 µs | 0.80× | 42 | 79 |
| regex/match 1000 | 47.00 µs | 151.66 µs | 0.31× | 0 | 0 |

23 of 27 benchmarks are slower than Go.

## Compiling

`compile.to` measures what the compiler costs (docs/speed.md, A1): `tondo
check`, `tondo build`, a cold `tondo run`, with no build in the cache but
the runtime's archives there, and a warm one, for the programs of
`compile/` and for projects of about 3,000, 24,000 and 192,000 lines that
`compile/generate.to` writes; and the first build after installing. A row
is the median of its runs in wall time and, where `perf` counts them, in
instructions of user code, which layout and load move far less. `TONDO`
names the `tondo` it measures:

```sh
TONDO=target/release/tondo tondo run bench/compile.to -- --runs 5 --output compile.md
TONDO=target/release/tondo tondo run bench/compile.to -- --old <tondo before>
TONDO=target/release/tondo tondo run bench/compile.to -- --guard
```

`--old` measures another `tondo` in turns with this one, and fails on a row
that costs more than 5% more. `--guard`, which the Nightly runs, fails when
the generated project of 8 times the lines costs more than 10 times as
much, and names the part of the compiler that grew the most:
`TONDO_COMPILE_TIMES=1` makes `tondo` say how long each part takes.
