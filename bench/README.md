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

The tables were measured on an AMD Ryzen 9 5950X, Linux, with Go 1.27 and
.NET 10, on 2026-11-14 at e66c13bf, each side for its default level of
processor: Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`. A row is
marked when it is slower than Go, and its allocations when it allocates
more, in number or in bytes:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 365.52 µs | 6.16 µs | 59.37× ⚠ | 2 ⚠ | 0 | 80 | 0 | 4.36M | 80.0k | 1.75M | 30.1k |
| queries/numbers iter map filter sum 10000 | 67.69 µs | 7.12 µs | 9.51× ⚠ | 3 ⚠ | 0 | 120 | 0 | 1.31M | 142.8k | 323.8k | 34.2k |
| texts/trim | 30.1 ns | 5.3 ns | 5.72× ⚠ | 1 ⚠ | 0 | 67 | 0 | 542 | 115 | 138 | 25 |
| texts/chars ascii | 1.60 µs | 305.6 ns | 5.24× ⚠ | 0 | 0 | 0 | 0 | 44.1k | 9.9k | 7.8k | 1.5k |
| ffi/from C threads 8x1000 | 760.57 µs | 172.81 µs | 4.40× ⚠ | 8041 ⚠ | 0 | 3.42M | 4 | 13.1k | 4.12M | 34.4k | 1.49M |
| texts/parse ints 1000 | 15.62 µs | 6.39 µs | 2.44× ⚠ | 0 | 0 | 0 | 0 | 399.4k | 144.4k | 75.6k | 30.8k |
| texts/chars other | 2.58 µs | 1.12 µs | 2.31× ⚠ | 0 | 0 | 0 | 0 | 63.0k | 28.2k | 12.4k | 5.6k |
| queries/countBy 10000 | 238.46 µs | 110.68 µs | 2.15× ⚠ | 6 ⚠ | 2 | 304 | 256 | 2.98M | 1.56M | 1.25M | 541.9k |
| ffi/from a C thread 1000 | 125.29 µs | 60.77 µs | 2.06× ⚠ | 1005 ⚠ | 0 | 438.2k | 0 | 1.8k | 516.0k | 4.2k | 163.2k |
| ffi/waits from a C thread 100 | 1.32 ms | 1.01 ms | 1.30× ⚠ | 306 ⚠ | 200 | 108.7k | 12.5k | 339.1k | 2.24M | 678.2k | 2.37M |
| queries/maxBy 10000 | 9.52 µs | 8.38 µs | 1.14× ⚠ | 1 ⚠ | 0 | 40 | 0 | 225.4k | 150.1k | 45.6k | 39.6k |
| texts/toUpper other | 8.64 µs | 8.19 µs | 1.05× ⚠ | 2 ⚠ | 1 | 3.4k | 1.8k | 136.4k | 173.1k | 42.1k | 38.3k |
| net/tls echo 1x1000 | 12.77 ms | 12.19 ms | 1.05× ⚠ | 7000 ⚠ | 3744 | 172.1k | 50.6k | 18.69M | 14.22M | 14.38M | 12.03M |
| tasks/spawn 1000 | 177.21 µs | 171.12 µs | 1.04× ⚠ | 1952 ⚠ | 2002 | 626.5k | 78.5k | 2.25M | 2.32M | 1.36M | 1.60M |
| floats/sqrt 100000 | 178.07 µs | 174.59 µs | 1.02× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.1k | 887.4k | 876.1k |
| queries/uniqueBy 10000 | 151.06 µs | 150.93 µs | 1.00× ⚠ | 40 ⚠ | 32 | 128.2k | 160.2k | 2.21M | 1.96M | 721.8k | 770.9k |
| net/echo 1x1000 | 11.26 ms | 11.28 ms | 1.00× | 3000 ⚠ | 3744 | 94.4k | 50.5k | 10.65M | 7.11M | 8.95M | 7.40M |
| floats/mandelbrot 64 | 137.79 µs | 138.65 µs | 0.99× | 0 | 0 | 0 | 0 | 1.49M | 1.56M | 681.7k | 684.7k |
| calls/closure 1000 | 203.5 ns | 207.2 ns | 0.98× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| strings/split 100 | 764.4 ns | 782.9 ns | 0.98× | 1 ⚠ | 1 | 4.4k | 1.8k | 16.0k | 13.8k | 3.7k | 3.9k |
| runtime/mutex 4x1000 | 20.32 µs | 21.12 µs | 0.96× | 6 ⚠ | 6 | 432 | 224 | 408.9k | 121.9k | 94.7k | 153.1k |
| arrays/index sum 10000 | 2.07 µs | 2.19 µs | 0.95× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.7k |
| runtime/select 1000 | 134.00 µs | 143.64 µs | 0.93× | 4 ⚠ | 4 | 723 | 528 | 1.34M | 2.09M | 597.6k | 737.0k |
| strings/builder 100 | 740.1 ns | 794.1 ns | 0.93× | 6 | 8 | 1.2k | 1.9k | 16.2k | 13.5k | 3.5k | 4.1k |
| texts/split other | 1.53 µs | 1.65 µs | 0.93× | 1 ⚠ | 1 | 9.4k | 3.4k | 31.8k | 27.3k | 7.3k | 7.8k |
| json/decode struct | 1.14 µs | 1.24 µs | 0.92× | 2 | 4 | 280 | 288 | 18.0k | 21.0k | 5.5k | 6.0k |
| loops/collatz 10000 | 735.56 µs | 839.33 µs | 0.88× | 0 | 0 | 0 | 0 | 15.37M | 8.86M | 3.60M | 4.14M |
| http/hello 1x1000 | 33.93 ms | 40.06 ms | 0.85× | 67000 ⚠ | 69147 | 20.36M | 6.18M | 71.31M | 148.65M | 87.19M | 145.22M |
| loops/sum 1000000 | 610.74 µs | 728.97 µs | 0.84× | 0 | 0 | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.57M |
| runtime/join 1 | 255.0 ns | 306.7 ns | 0.83× | 1 | 2 | 40 | 144 | 2.8k | 3.2k | 1.2k | 1.4k |
| strings/contains 1000 | 13.1 ns | 16.1 ns | 0.81× | 0 | 0 | 0 | 0 | 332 | 394 | 63 | 76 |
| http/tls 64x100 | 66.47 ms | 82.72 ms | 0.80× | 494319 ⚠ | 459887 | 57.62M | 51.45M | 610.89M | 1.87G | 952.10M | 1.74G |
| json/encode 100 structs | 23.63 µs | 30.21 µs | 0.78× | 2 | 4 | 7.2k | 16.1k | 406.6k | 442.1k | 113.1k | 144.0k |
| texts/join 100 | 507.4 ns | 658.2 ns | 0.77× | 1 ⚠ | 1 | 1.0k | 576 | 8.2k | 11.5k | 2.4k | 3.1k |
| calls/fib 25 | 216.26 µs | 288.04 µs | 0.75× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.05M | 1.38M |
| maps/lookup 10000 ints | 53.99 µs | 74.28 µs | 0.73× | 0 | 0 | 0 | 0 | 970.1k | 1.06M | 258.2k | 359.6k |
| tasks/channel 10000 | 195.66 µs | 270.28 µs | 0.72× | 2 ⚠ | 2 | 744 | 664 | 3.17M | 4.41M | 924.8k | 1.32M |
| queries/joinBy 10000x1000 | 522.40 µs | 733.68 µs | 0.71× | 1047 ⚠ | 1039 | 1.67M | 2.91M | 6.42M | 13.07M | 2.52M | 5.89M |
| strings/int to text 1000 | 7.96 µs | 11.25 µs | 0.71× | 0 | 900 | 0 | 2.8k | 163.6k | 211.4k | 38.3k | 55.0k |
| strings/interpolate | 23.0 ns | 33.5 ns | 0.69× | 1 ⚠ | 1 | 52 | 32 | 495 | 664 | 110 | 158 |
| http/json 64x100 | 55.40 ms | 84.88 ms | 0.65× | 672500 ⚠ | 702829 | 962.96M | 97.55M | 763.71M | 3.57G | 2.48G | 6.12G |
| json/parse object | 2.33 µs | 3.75 µs | 0.62× | 17 ⚠ | 36 | 2.3k | 1.5k | 35.0k | 48.4k | 11.2k | 17.7k |
| texts/replace ascii | 714.1 ns | 1.30 µs | 0.55× | 9 ⚠ | 1 | 4.6k | 1.0k | 14.3k | 24.6k | 3.4k | 6.0k |
| queries/filter sumBy 10000 | 73.22 µs | 137.25 µs | 0.53× | 12 | 16 | 512.2k | 727.7k | 876.9k | 2.41M | 347.8k | 1.16M |
| queries/groupBy 10000 | 282.69 µs | 530.42 µs | 0.53× | 2683 | 4664 | 879.9k | 1.18M | 3.74M | 7.87M | 1.35M | 3.64M |
| arrays/map filter sum 10000 | 17.26 µs | 32.92 µs | 0.52× | 12 | 16 | 142.5k | 165.2k | 374.2k | 490.3k | 82.4k | 189.7k |
| json/parse 1000 numbers | 72.42 µs | 138.39 µs | 0.52× | 909 ⚠ | 2015 | 55.9k | 54.2k | 1.38M | 2.12M | 348.2k | 641.4k |
| structs/users 1000 | 19.57 µs | 39.29 µs | 0.50× | 1001 | 1910 | 48.3k | 72.8k | 408.0k | 669.3k | 93.8k | 186.9k |
| queries/averageBy 10000 | 11.33 µs | 24.08 µs | 0.47× | 1 ⚠ | 0 | 40 | 0 | 212.8k | 90.1k | 54.4k | 120.3k |
| queries/numbers map filter sum 10000 | 16.73 µs | 36.30 µs | 0.46× | 12 | 16 | 142.5k | 165.2k | 374.2k | 654.1k | 79.8k | 281.6k |
| texts/replace other | 524.6 ns | 1.16 µs | 0.45× | 3 ⚠ | 1 | 6.8k | 1.8k | 11.9k | 19.6k | 2.5k | 5.4k |
| maps/insert 1000 strings | 43.57 µs | 100.87 µs | 0.43× | 1030 | 1920 | 111.6k | 121.0k | 679.5k | 1.30M | 209.4k | 458.7k |
| queries/sortedBy 10000 | 1.61 ms | 3.88 ms | 0.42× | 6 ⚠ | 2 | 1.45M | 392.0k | 23.47M | 30.36M | 7.73M | 19.30M |
| ffi/callback 1000 | 11.94 µs | 30.22 µs | 0.39× | 0 | 0 | 0 | 0 | 172.3k | 497.4k | 57.3k | 138.4k |
| queries/toSet 10000 | 194.38 µs | 494.35 µs | 0.39× | 42 ⚠ | 81 | 1024.0k | 577.8k | 2.42M | 5.87M | 939.5k | 2.64M |
| texts/contains other | 180.9 ns | 471.1 ns | 0.38× | 0 | 0 | 0 | 0 | 3.5k | 8.2k | 873 | 2.3k |
| queries/keyedBy 10000 | 210.48 µs | 699.73 µs | 0.30× | 42 ⚠ | 81 | 2.00M | 1.75M | 3.22M | 9.31M | 1.01M | 4.49M |
| maps/insert 10000 ints | 133.22 µs | 500.38 µs | 0.27× | 42 ⚠ | 79 | 1.25M | 577.6k | 2.33M | 5.39M | 640.0k | 2.34M |
| regex/match 1000 | 39.42 µs | 152.44 µs | 0.26× | 0 | 0 | 0 | 0 | 573.3k | 2.99M | 191.7k | 733.6k |
| http/hello 64x100 | 20.60 ms | 80.87 ms | 0.25× | 429067 ⚠ | 447660 | 130.06M | 43.75M | 454.11M | 3.05G | 1.17G | 5.42G |
| floats/add 100000 | 20.34 µs | 95.46 µs | 0.21× | 1 | 1 | 781.3k | 784.0k | 281.4k | 1.70M | 96.2k | 545.1k |
| structs/points 1000 | 1.58 µs | 7.69 µs | 0.21× | 1 | 12 | 15.7k | 49.2k | 34.7k | 81.5k | 7.6k | 44.9k |
| arrays/sort 10000 | 66.87 µs | 361.62 µs | 0.18× | 1 | 1 | 78.2k | 80.0k | 1.37M | 2.42M | 320.3k | 1.72M |
| tensors/matmul 64 | 82.19 µs | 491.28 µs | 0.17× | 2 ⚠ | 1 | 32.1k | 32.0k | 1.59M | 4.30M | 397.8k | 2.42M |
| tensors/broadcast 256 | 10.24 µs | 71.46 µs | 0.14× | 2 ⚠ | 1 | 512.1k | 512.0k | 189.7k | 1.45M | 49.2k | 488.8k |
| arrays/push 1000 | 503.9 ns | 3.59 µs | 0.14× | 1 | 12 | 7.8k | 24.6k | 13.1k | 42.8k | 2.4k | 23.6k |
| floats/dot 100000 | 16.24 µs | 240.66 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 1.00M | 78.5k | 1.21M |
| floats/sum 100000 | 7.93 µs | 240.30 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 599.7k | 38.4k | 1.20M |
| texts/toUpper ascii | 74.9 ns | 2.38 µs | 0.03× | 2 ⚠ | 1 | 2.2k | 1.1k | 1.9k | 51.6k | 366 | 11.4k |

16 of 69 benchmarks are slower than Go; 37 allocate more; 20 run more instructions than Go.

The rows still behind Go, and why:

- `queries/iter filter sumBy`, `queries/numbers iter map filter sum`: a
  lazy chain calls each step's closure through the iterator's record, and
  `Iterator.sumBy` is too large to inline into the caller, so nothing
  resolves those calls; the eager chains, which inline, are at half Go's.
  Following `fn` values through a record into the code that calls them is
  the next work of change 33.
- `texts/trim`: a trimmed text is a new one holding its bytes, where Go's
  is a view of the old; texts short enough to live in a frame (change 33,
  rule 8) are what would remove it.
- `texts/chars ascii` and `other`: each character goes through the
  iterator's record in memory, with the checks of a call; Go's loop keeps
  its offset in a register.
- `texts/parse ints`: a call of the runtime for each number, which counts
  in 128 bits.
- `queries/countBy`: each element's key takes two calls of the runtime,
  one that finds its entry and one for where the entries start, to count
  its position, where Go's takes one; the hash itself takes a word at a
  time.
- `queries/maxBy`: `Array.maxBy` is too large to inline (40), so its
  closure is made on the heap, an allocation a run, and called for each
  element.
- `ffi/*`: change 30; the target is Tony's to set.
- `texts/toUpper other`, `queries/uniqueBy`, `floats/sqrt`, `tasks/spawn`:
  within 5% of Go, as much as a row moves from one run to the next.
- `net/tls echo` makes about three more allocations a round trip than Go,
  and `http/tls` a few more a request: rustls 0.23 makes a buffer for each
  record it encrypts and copies each one it reads into another, in its
  buffered API and its unbuffered one alike.

`B/op` counts what each side's allocator gives. In `tasks/spawn` Tondo
allocates each task's record with its join, about 600 bytes, where Go takes
its goroutines' from a list of those that ended. Where Tondo allocates as
many times as Go but more bytes, as `strings/split`, its pieces are texts of
their own, each with a header and its bytes, where Go's are views of the
first. `http/json` makes as many allocations as Go and ten times the bytes,
about 150 KB a request, which is still to be found.

The servers of the `http` rows at 20000 requests a second, each sent when
due whether or not those before came back, its latency from then until its
answer, after half a second that opens the connections (`bench/latency`):

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 52 µs | 285 µs | 86 µs | 1412 µs |
| http/json 20000/s | 106 µs | 152 µs | 166 µs | 1380 µs |
| http/tls hello 20000/s | 107 µs ⚠ | 90 µs | 173 µs | 1567 µs |

Tondo's median is behind Go's on `http/tls hello` alone, and its 99th
percentile is about a tenth of Go's on every server; why the median of
TLS is behind is still to be found.

The rows that LINQ says something of, against C# (`bench/csharp`, measured
as `go test -bench` measures, its bytes from the allocator and no count of
allocations):

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 365.52 µs | 77.60 µs | 4.71× ⚠ | 80 | 96 |
| queries/numbers iter map filter sum 10000 | 67.69 µs | 24.92 µs | 2.72× ⚠ | 120 | 128 |
| queries/countBy 10000 | 238.46 µs | 82.51 µs | 2.89× ⚠ | 304 | 1.1k |
| queries/maxBy 10000 | 9.52 µs | 16.64 µs | 0.57× | 40 | 40 |
| queries/uniqueBy 10000 | 151.06 µs | 61.08 µs | 2.47× ⚠ | 128.2k | 65.2k |
| queries/joinBy 10000x1000 | 522.40 µs | 137.05 µs | 3.81× ⚠ | 1.67M | 102.3k |
| queries/filter sumBy 10000 | 73.22 µs | 54.00 µs | 1.36× ⚠ | 512.2k | 39.0k |
| queries/groupBy 10000 | 282.69 µs | 273.92 µs | 1.03× ⚠ | 879.9k | 583.9k |
| queries/averageBy 10000 | 11.33 µs | 25.61 µs | 0.44× | 40 | 24 |
| queries/numbers map filter sum 10000 | 16.73 µs | 10.93 µs | 1.53× ⚠ | 142.5k | 51.7k |
| queries/sortedBy 10000 | 1.61 ms | 516.96 µs | 3.12× ⚠ | 1.45M | 234.7k |
| queries/toSet 10000 | 194.38 µs | 47.62 µs | 4.08× ⚠ | 1024.0k | 158.0k |
| queries/keyedBy 10000 | 210.48 µs | 72.94 µs | 2.89× ⚠ | 2.00M | 276.4k |

C# is ahead on every query row but `maxBy` and `averageBy`: four and three
times on the lazy chains, and two to four times on sorting, on sets and on
most maps, `groupBy` even.

Both for x86-64-v3, `--cpu x86-64-v3` and `GOAMD64=v3`, with
`compare.to --cpu x86-64-v3`, the same day:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 370.77 µs | 6.19 µs | 59.94× ⚠ | 2 ⚠ | 0 | 80 | 0 | 4.36M | 80.0k | 1.79M | 30.0k |
| queries/numbers iter map filter sum 10000 | 73.71 µs | 7.06 µs | 10.44× ⚠ | 3 ⚠ | 0 | 120 | 0 | 1.31M | 142.8k | 344.6k | 34.0k |
| texts/chars ascii | 1.61 µs | 306.3 ns | 5.24× ⚠ | 0 | 0 | 0 | 0 | 44.1k | 9.9k | 7.8k | 1.5k |
| texts/trim | 23.8 ns | 5.2 ns | 4.58× ⚠ | 1 ⚠ | 0 | 67 | 0 | 542 | 115 | 114 | 25 |
| ffi/from C threads 8x1000 | 759.00 µs | 172.14 µs | 4.41× ⚠ | 8041 ⚠ | 0 | 3.42M | 4 | 13.1k | 4.12M | 34.1k | 1.49M |
| texts/parse ints 1000 | 15.40 µs | 6.40 µs | 2.41× ⚠ | 0 | 0 | 0 | 0 | 399.4k | 144.4k | 74.4k | 32.8k |
| queries/countBy 10000 | 234.01 µs | 106.33 µs | 2.20× ⚠ | 6 ⚠ | 2 | 304 | 256 | 2.95M | 1.54M | 1.13M | 508.0k |
| texts/chars other | 2.47 µs | 1.19 µs | 2.08× ⚠ | 0 | 0 | 0 | 0 | 63.0k | 28.2k | 11.8k | 5.3k |
| ffi/from a C thread 1000 | 125.57 µs | 61.22 µs | 2.05× ⚠ | 1005 ⚠ | 0 | 438.2k | 0 | 1.8k | 515.6k | 4.1k | 161.6k |
| ffi/waits from a C thread 100 | 1.29 ms | 1.05 ms | 1.24× ⚠ | 306 ⚠ | 200 | 108.7k | 12.5k | 340.8k | 2.32M | 769.0k | 2.80M |
| queries/maxBy 10000 | 9.91 µs | 8.29 µs | 1.20× ⚠ | 1 ⚠ | 0 | 40 | 0 | 225.4k | 150.1k | 47.8k | 42.3k |
| texts/toUpper other | 8.66 µs | 8.09 µs | 1.07× ⚠ | 2 ⚠ | 1 | 3.4k | 1.8k | 136.4k | 172.4k | 42.1k | 38.8k |
| queries/uniqueBy 10000 | 156.21 µs | 146.88 µs | 1.06× ⚠ | 40 ⚠ | 32 | 128.2k | 160.2k | 2.21M | 1.91M | 754.5k | 740.8k |
| tasks/spawn 1000 | 178.89 µs | 169.01 µs | 1.06× ⚠ | 1952 ⚠ | 2002 | 626.3k | 78.5k | 2.24M | 2.27M | 1.36M | 1.31M |
| floats/mandelbrot 64 | 134.38 µs | 127.03 µs | 1.06× ⚠ | 0 | 0 | 0 | 0 | 1.20M | 1.42M | 668.0k | 625.6k |
| net/tls echo 1x1000 | 12.71 ms | 12.22 ms | 1.04× ⚠ | 7000 ⚠ | 3744 | 172.1k | 50.6k | 18.69M | 14.26M | 14.32M | 11.13M |
| loops/collatz 10000 | 865.40 µs | 837.49 µs | 1.03× ⚠ | 0 | 0 | 0 | 0 | 15.37M | 8.86M | 4.24M | 4.14M |
| floats/sqrt 100000 | 178.03 µs | 174.71 µs | 1.02× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.4k | 893.7k | 873.8k |
| strings/split 100 | 784.0 ns | 780.0 ns | 1.01× ⚠ | 1 ⚠ | 1 | 4.4k | 1.8k | 16.0k | 13.9k | 3.8k | 4.0k |
| net/echo 1x1000 | 11.32 ms | 11.30 ms | 1.00× ⚠ | 3000 ⚠ | 3744 | 94.4k | 50.5k | 10.65M | 7.12M | 8.95M | 7.49M |
| texts/split other | 1.53 µs | 1.58 µs | 0.97× | 1 ⚠ | 1 | 9.4k | 3.4k | 31.8k | 27.2k | 7.4k | 7.9k |
| calls/closure 1000 | 203.5 ns | 209.7 ns | 0.97× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| runtime/mutex 4x1000 | 20.42 µs | 21.25 µs | 0.96× | 6 ⚠ | 6 | 432 | 224 | 408.9k | 119.4k | 94.8k | 140.1k |
| json/decode struct | 1.22 µs | 1.27 µs | 0.96× | 2 | 4 | 280 | 288 | 18.0k | 21.0k | 5.9k | 6.1k |
| arrays/index sum 10000 | 2.07 µs | 2.22 µs | 0.93× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.9k |
| strings/builder 100 | 746.7 ns | 821.5 ns | 0.91× | 6 | 8 | 1.2k | 1.9k | 16.2k | 13.4k | 3.6k | 4.2k |
| runtime/select 1000 | 122.88 µs | 143.49 µs | 0.86× | 4 ⚠ | 4 | 723 | 528 | 1.34M | 2.10M | 545.9k | 726.4k |
| http/hello 1x1000 | 33.48 ms | 39.68 ms | 0.84× | 67000 ⚠ | 69145 | 20.36M | 6.18M | 71.32M | 144.80M | 83.50M | 135.90M |
| strings/contains 1000 | 13.2 ns | 15.7 ns | 0.84× | 0 | 0 | 0 | 0 | 332 | 394 | 63 | 76 |
| loops/sum 1000000 | 608.73 µs | 730.41 µs | 0.83× | 0 | 0 | 0 | 0 | 8.25M | 15.00M | 3.00M | 3.55M |
| texts/join 100 | 508.4 ns | 616.1 ns | 0.83× | 1 ⚠ | 1 | 1.0k | 576 | 8.2k | 11.5k | 2.4k | 2.9k |
| runtime/join 1 | 253.5 ns | 308.3 ns | 0.82× | 1 | 2 | 40 | 144 | 2.8k | 3.2k | 1.2k | 1.4k |
| maps/lookup 10000 ints | 54.69 µs | 69.87 µs | 0.78× | 0 | 0 | 0 | 0 | 970.1k | 1.01M | 261.6k | 335.1k |
| http/tls 64x100 | 65.32 ms | 83.77 ms | 0.78× | 494154 ⚠ | 459508 | 57.66M | 51.37M | 611.34M | 1.75G | 941.28M | 1.60G |
| json/encode 100 structs | 23.85 µs | 31.54 µs | 0.76× | 2 | 4 | 7.2k | 16.1k | 406.6k | 441.4k | 114.6k | 144.4k |
| queries/joinBy 10000x1000 | 525.98 µs | 701.05 µs | 0.75× | 1047 ⚠ | 1039 | 1.67M | 2.91M | 6.42M | 13.34M | 2.54M | 6.20M |
| calls/fib 25 | 231.63 µs | 325.05 µs | 0.71× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.13M | 1.41M |
| strings/int to text 1000 | 8.22 µs | 11.67 µs | 0.70× | 0 | 900 | 0 | 2.8k | 163.6k | 211.4k | 39.4k | 55.6k |
| strings/interpolate | 22.9 ns | 33.2 ns | 0.69× | 1 ⚠ | 1 | 52 | 32 | 495 | 662 | 109 | 157 |
| tasks/channel 10000 | 191.27 µs | 280.75 µs | 0.68× | 2 ⚠ | 2 | 744 | 664 | 3.17M | 4.41M | 906.0k | 1.34M |
| http/json 64x100 | 53.29 ms | 78.54 ms | 0.68× | 672460 ⚠ | 700107 | 962.96M | 90.09M | 750.53M | 3.62G | 2.44G | 6.12G |
| json/parse object | 2.31 µs | 3.84 µs | 0.60× | 17 ⚠ | 36 | 2.3k | 1.5k | 35.0k | 47.7k | 11.1k | 18.6k |
| json/parse 1000 numbers | 72.70 µs | 131.28 µs | 0.55× | 909 ⚠ | 2015 | 55.9k | 54.2k | 1.38M | 2.12M | 349.7k | 618.6k |
| texts/replace ascii | 681.1 ns | 1.25 µs | 0.54× | 9 ⚠ | 1 | 4.6k | 1.0k | 14.3k | 24.6k | 3.3k | 6.0k |
| queries/groupBy 10000 | 281.53 µs | 524.34 µs | 0.54× | 2683 | 4664 | 879.9k | 1.18M | 3.74M | 7.76M | 1.35M | 3.64M |
| queries/filter sumBy 10000 | 73.46 µs | 139.23 µs | 0.53× | 12 | 16 | 512.2k | 727.7k | 876.9k | 2.46M | 356.0k | 1.22M |
| arrays/map filter sum 10000 | 16.92 µs | 32.39 µs | 0.52× | 12 | 16 | 142.5k | 165.2k | 374.2k | 481.4k | 81.3k | 193.2k |
| structs/users 1000 | 18.88 µs | 39.79 µs | 0.47× | 1001 | 1910 | 48.3k | 72.8k | 404.8k | 668.4k | 90.3k | 204.2k |
| queries/averageBy 10000 | 11.35 µs | 24.20 µs | 0.47× | 1 ⚠ | 0 | 40 | 0 | 212.8k | 89.8k | 54.7k | 120.9k |
| queries/numbers map filter sum 10000 | 16.71 µs | 36.01 µs | 0.46× | 12 | 16 | 142.5k | 165.2k | 374.2k | 659.4k | 80.2k | 288.9k |
| maps/insert 1000 strings | 46.63 µs | 100.59 µs | 0.46× | 1030 | 1920 | 111.6k | 121.0k | 675.5k | 1.30M | 223.9k | 457.8k |
| texts/replace other | 524.0 ns | 1.18 µs | 0.44× | 3 ⚠ | 1 | 6.8k | 1.8k | 11.9k | 19.5k | 2.5k | 5.8k |
| ffi/callback 1000 | 12.97 µs | 29.78 µs | 0.44× | 0 | 0 | 0 | 0 | 172.3k | 497.4k | 62.7k | 160.4k |
| queries/sortedBy 10000 | 1.60 ms | 3.90 ms | 0.41× | 6 ⚠ | 2 | 1.45M | 392.0k | 23.47M | 32.27M | 7.70M | 19.43M |
| queries/toSet 10000 | 196.71 µs | 488.13 µs | 0.40× | 42 ⚠ | 81 | 1024.0k | 577.8k | 2.42M | 5.86M | 945.3k | 2.57M |
| texts/contains other | 178.0 ns | 473.2 ns | 0.38× | 0 | 0 | 0 | 0 | 3.5k | 8.2k | 860 | 2.3k |
| queries/keyedBy 10000 | 212.75 µs | 694.39 µs | 0.31× | 42 ⚠ | 81 | 2.00M | 1.75M | 3.22M | 9.33M | 1.02M | 4.42M |
| http/hello 64x100 | 20.53 ms | 72.13 ms | 0.28× | 429243 ⚠ | 445352 | 130.31M | 43.04M | 454.36M | 3.04G | 1.20G | 5.20G |
| maps/insert 10000 ints | 136.52 µs | 492.68 µs | 0.28× | 42 ⚠ | 79 | 1.25M | 577.6k | 2.33M | 5.33M | 655.2k | 2.31M |
| regex/match 1000 | 38.42 µs | 153.07 µs | 0.25× | 0 | 0 | 0 | 0 | 573.3k | 2.99M | 186.3k | 740.2k |
| structs/points 1000 | 1.57 µs | 7.59 µs | 0.21× | 1 | 12 | 15.7k | 49.2k | 34.7k | 86.4k | 7.6k | 48.2k |
| floats/add 100000 | 18.13 µs | 97.80 µs | 0.19× | 1 | 1 | 781.3k | 784.0k | 281.4k | 1.69M | 86.8k | 604.1k |
| arrays/sort 10000 | 66.54 µs | 365.67 µs | 0.18× | 1 | 1 | 78.2k | 80.0k | 1.37M | 2.41M | 321.2k | 1.73M |
| tensors/matmul 64 | 83.58 µs | 495.28 µs | 0.17× | 2 ⚠ | 1 | 32.1k | 32.0k | 1.59M | 4.30M | 405.2k | 2.40M |
| tensors/broadcast 256 | 10.25 µs | 71.06 µs | 0.14× | 2 ⚠ | 1 | 512.1k | 512.0k | 189.7k | 1.45M | 49.3k | 523.2k |
| arrays/push 1000 | 507.1 ns | 3.62 µs | 0.14× | 1 | 12 | 7.8k | 24.6k | 13.1k | 41.4k | 2.4k | 24.6k |
| floats/dot 100000 | 16.29 µs | 241.96 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 1.00M | 78.6k | 1.20M |
| floats/sum 100000 | 7.91 µs | 240.30 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 599.6k | 38.3k | 1.20M |
| texts/toUpper ascii | 78.8 ns | 2.41 µs | 0.03× | 2 ⚠ | 1 | 2.2k | 1.1k | 1.9k | 51.6k | 379 | 11.5k |

20 of 69 benchmarks are slower than Go; 37 allocate more; 20 run more instructions than Go.

The servers at this level:

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 58 µs | 287 µs | 84 µs | 1403 µs |
| http/json 20000/s | 112 µs | 213 µs | 172 µs | 1456 µs |
| http/tls hello 20000/s | 110 µs ⚠ | 89 µs | 176 µs | 1556 µs |

The same rows are behind at this level, and four more: Go makes more of
FMA in `floats/mandelbrot`; `loops/collatz`, whose code is the same at both
levels, is where its code lies, from 736 µs to 871 µs over four orders of
the functions at `baseline` when last measured so; `strings/split` and
`net/echo` are within 1% of Go.

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
