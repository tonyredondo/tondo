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
and one with more instructions does more work. Neither side's time pays
for the counting, which slows every switch of a thread it counts: Tondo
counts in a round of its own after its samples, as Go in runs of its own.

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
.NET 10, on 2026-11-16 at 0230fe0f, each side for its default level of
processor: Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`. A row is
marked when it is slower than Go, and its allocations when it allocates
more, in number or in bytes:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 50.93 µs | 6.24 µs | 8.16× ⚠ | 1 ⚠ | 0 | 40 | 0 | 540.1k | 80.0k | 248.2k | 29.9k |
| texts/trim | 23.8 ns | 5.2 ns | 4.56× ⚠ | 1 ⚠ | 0 | 67 | 0 | 540 | 115 | 114 | 25 |
| queries/numbers iter map filter sum 10000 | 19.21 µs | 7.09 µs | 2.71× ⚠ | 2 ⚠ | 0 | 80 | 0 | 414.9k | 142.8k | 92.9k | 34.0k |
| texts/chars ascii | 461.9 ns | 306.5 ns | 1.51× ⚠ | 0 | 0 | 0 | 0 | 11.1k | 9.9k | 2.3k | 1.5k |
| texts/chars other | 1.71 µs | 1.15 µs | 1.48× ⚠ | 0 | 0 | 0 | 0 | 33.4k | 28.2k | 8.3k | 5.4k |
| queries/countBy 10000 | 143.87 µs | 110.32 µs | 1.30× ⚠ | 6 ⚠ | 2 | 304 | 256 | 2.52M | 1.55M | 692.0k | 530.5k |
| texts/parse ints 1000 | 8.20 µs | 6.41 µs | 1.28× ⚠ | 0 | 0 | 0 | 0 | 194.6k | 144.5k | 39.8k | 30.9k |
| tasks/spawn 1000 | 194.66 µs | 168.82 µs | 1.15× ⚠ | 1954 ⚠ | 2002 | 641.7k | 78.5k | 2.22M | 2.35M | 1.43M | 1.66M |
| texts/toUpper other | 8.81 µs | 8.00 µs | 1.10× ⚠ | 2 ⚠ | 1 | 3.4k | 1.8k | 136.4k | 172.9k | 42.8k | 38.2k |
| ffi/from a C thread 1000 | 66.31 µs | 60.92 µs | 1.09× ⚠ | 2 ⚠ | 0 | 617 | 0 | 1.8k | 516.9k | 4.4k | 159.6k |
| ffi/from C threads 8x1000 | 182.14 µs | 172.37 µs | 1.06× ⚠ | 16 ⚠ | 0 | 4.8k | 5 | 12.9k | 4.12M | 36.4k | 1.48M |
| net/tls echo 1x1000 | 12.87 ms | 12.24 ms | 1.05× ⚠ | 7000 ⚠ | 3745 | 172.1k | 50.7k | 18.68M | 14.12M | 14.87M | 11.24M |
| floats/sqrt 100000 | 177.00 µs | 174.15 µs | 1.02× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.0k | 886.8k | 866.7k |
| queries/uniqueBy 10000 | 150.82 µs | 148.77 µs | 1.01× ⚠ | 39 ⚠ | 32 | 128.1k | 160.2k | 2.22M | 1.95M | 731.1k | 773.9k |
| net/echo 1x1000 | 11.14 ms | 11.34 ms | 0.98× | 3000 ⚠ | 3744 | 94.4k | 50.5k | 10.66M | 7.13M | 9.03M | 7.18M |
| calls/closure 1000 | 202.6 ns | 206.7 ns | 0.98× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| loops/collatz 10000 | 815.66 µs | 836.66 µs | 0.97× | 0 | 0 | 0 | 0 | 15.37M | 8.86M | 4.00M | 4.14M |
| floats/mandelbrot 64 | 134.79 µs | 138.49 µs | 0.97× | 0 | 0 | 0 | 0 | 1.48M | 1.57M | 671.4k | 687.8k |
| runtime/mutex 4x1000 | 20.20 µs | 20.81 µs | 0.97× | 6 ⚠ | 6 | 432 | 224 | 408.6k | 120.7k | 94.6k | 146.4k |
| texts/split other | 1.52 µs | 1.60 µs | 0.95× | 1 ⚠ | 1 | 9.4k | 3.4k | 31.8k | 27.3k | 7.3k | 7.9k |
| arrays/index sum 10000 | 2.06 µs | 2.17 µs | 0.95× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.4k |
| strings/split 100 | 754.1 ns | 801.2 ns | 0.94× | 1 ⚠ | 1 | 4.4k | 1.8k | 16.0k | 14.0k | 3.6k | 3.9k |
| json/decode struct | 1.17 µs | 1.24 µs | 0.94× | 2 | 4 | 280 | 288 | 17.9k | 21.0k | 5.6k | 5.9k |
| strings/builder 100 | 734.8 ns | 797.7 ns | 0.92× | 6 | 8 | 1.2k | 1.9k | 16.3k | 13.4k | 3.5k | 3.7k |
| strings/contains 1000 | 13.5 ns | 15.6 ns | 0.86× | 0 | 0 | 0 | 0 | 340 | 394 | 65 | 75 |
| runtime/select 1000 | 123.55 µs | 144.15 µs | 0.86× | 4 ⚠ | 4 | 723 | 528 | 1.33M | 2.11M | 549.5k | 755.7k |
| runtime/join 1 | 259.5 ns | 304.3 ns | 0.85× | 1 | 2 | 40 | 144 | 2.8k | 3.2k | 1.2k | 1.4k |
| loops/sum 1000000 | 606.50 µs | 729.09 µs | 0.83× | 0 | 0 | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.58M |
| http/hello 1x1000 | 31.73 ms | 39.81 ms | 0.80× | 56000 ⚠ | 69145 | 19.57M | 6.18M | 67.90M | 145.54M | 77.94M | 137.91M |
| queries/maxBy 10000 | 6.66 µs | 8.37 µs | 0.80× | 0 | 0 | 0 | 0 | 145.3k | 150.1k | 32.6k | 39.7k |
| json/encode 100 structs | 23.26 µs | 29.89 µs | 0.78× | 2 | 4 | 7.2k | 16.1k | 404.0k | 441.9k | 112.3k | 141.7k |
| texts/join 100 | 510.5 ns | 664.6 ns | 0.77× | 1 ⚠ | 1 | 1.0k | 576 | 8.2k | 11.5k | 2.5k | 3.1k |
| calls/fib 25 | 216.42 µs | 288.40 µs | 0.75× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.05M | 1.41M |
| maps/lookup 10000 ints | 55.64 µs | 74.29 µs | 0.75× | 0 | 0 | 0 | 0 | 970.1k | 1.06M | 267.2k | 355.5k |
| tasks/channel 10000 | 192.73 µs | 272.81 µs | 0.71× | 2 ⚠ | 2 | 744 | 664 | 3.17M | 4.42M | 943.9k | 1.29M |
| strings/int to text 1000 | 7.92 µs | 11.30 µs | 0.70× | 0 | 900 | 0 | 2.8k | 163.6k | 211.7k | 38.3k | 54.9k |
| http/tls 64x100 | 56.87 ms | 81.67 ms | 0.70× | 423505 ⚠ | 459142 | 52.49M | 51.24M | 587.45M | 1.73G | 822.70M | 1.48G |
| strings/interpolate | 22.7 ns | 33.6 ns | 0.68× | 1 ⚠ | 1 | 52 | 32 | 497 | 664 | 108 | 160 |
| ffi/waits from a C thread 100 | 680.89 µs | 1.01 ms | 0.67× | 109 | 200 | 5.3k | 12.5k | 228.7k | 2.03M | 451.9k | 2.16M |
| json/parse object | 2.33 µs | 3.71 µs | 0.63× | 17 ⚠ | 36 | 2.3k | 1.5k | 34.9k | 48.2k | 11.3k | 18.1k |
| queries/joinBy 10000x1000 | 454.21 µs | 745.49 µs | 0.61× | 1045 ⚠ | 1039 | 1.67M | 2.91M | 6.23M | 13.48M | 2.18M | 6.25M |
| json/parse 1000 numbers | 75.98 µs | 132.34 µs | 0.57× | 909 ⚠ | 2015 | 55.9k | 54.2k | 1.38M | 2.12M | 368.1k | 646.2k |
| texts/replace ascii | 685.3 ns | 1.28 µs | 0.54× | 9 ⚠ | 1 | 4.6k | 1.0k | 14.3k | 24.6k | 3.3k | 6.1k |
| structs/users 1000 | 20.24 µs | 38.22 µs | 0.53× | 1001 | 1910 | 48.3k | 72.8k | 410.8k | 669.7k | 97.4k | 187.9k |
| queries/groupBy 10000 | 265.86 µs | 530.74 µs | 0.50× | 2683 | 4664 | 879.9k | 1.18M | 3.52M | 7.59M | 1.28M | 3.57M |
| queries/filter sumBy 10000 | 67.47 µs | 139.31 µs | 0.48× | 12 | 16 | 512.2k | 727.7k | 822.2k | 2.62M | 327.9k | 1.27M |
| arrays/map filter sum 10000 | 17.01 µs | 35.82 µs | 0.47× | 12 | 16 | 142.5k | 165.2k | 374.3k | 486.2k | 82.2k | 200.8k |
| texts/replace other | 532.8 ns | 1.13 µs | 0.47× | 3 ⚠ | 1 | 6.8k | 1.8k | 12.0k | 19.6k | 2.6k | 5.4k |
| queries/numbers map filter sum 10000 | 16.76 µs | 35.97 µs | 0.47× | 12 | 16 | 142.5k | 165.2k | 374.3k | 666.5k | 81.7k | 291.0k |
| queries/averageBy 10000 | 10.56 µs | 24.29 µs | 0.43× | 0 | 0 | 0 | 0 | 200.2k | 90.1k | 51.3k | 120.0k |
| http/json 64x100 | 30.90 ms | 76.38 ms | 0.40× | 518760 ⚠ | 698440 | 138.72M | 86.36M | 658.48M | 3.62G | 1.77G | 6.09G |
| maps/insert 1000 strings | 40.14 µs | 99.90 µs | 0.40× | 1030 | 1920 | 111.6k | 121.0k | 664.8k | 1.29M | 193.8k | 459.0k |
| queries/toSet 10000 | 194.86 µs | 493.07 µs | 0.40× | 42 ⚠ | 81 | 1024.0k | 577.8k | 2.43M | 5.87M | 943.0k | 2.66M |
| queries/sortedBy 10000 | 1.53 ms | 3.89 ms | 0.39× | 4 ⚠ | 2 | 1.45M | 392.0k | 18.95M | 33.36M | 7.38M | 19.52M |
| texts/contains other | 177.1 ns | 478.9 ns | 0.37× | 0 | 0 | 0 | 0 | 3.5k | 8.2k | 857 | 2.4k |
| ffi/callback 1000 | 9.97 µs | 29.89 µs | 0.33× | 0 | 0 | 0 | 0 | 140.2k | 497.4k | 48.2k | 147.1k |
| http/hello 64x100 | 19.28 ms | 58.32 ms | 0.33× | 358545 ⚠ | 442958 | 124.78M | 41.91M | 429.82M | 3.18G | 746.22M | 6.20G |
| queries/keyedBy 10000 | 207.04 µs | 697.24 µs | 0.30× | 42 ⚠ | 81 | 2.00M | 1.75M | 3.23M | 9.36M | 990.6k | 4.41M |
| maps/insert 10000 ints | 135.08 µs | 505.08 µs | 0.27× | 42 ⚠ | 79 | 1.25M | 577.6k | 2.34M | 5.37M | 652.0k | 2.30M |
| regex/match 1000 | 37.66 µs | 152.30 µs | 0.25× | 0 | 0 | 0 | 0 | 571.3k | 2.99M | 183.2k | 746.6k |
| floats/add 100000 | 20.48 µs | 97.48 µs | 0.21× | 1 | 1 | 781.3k | 784.0k | 281.3k | 1.70M | 86.8k | 607.0k |
| structs/points 1000 | 1.33 µs | 7.06 µs | 0.19× | 1 | 12 | 15.7k | 49.2k | 30.7k | 85.1k | 6.5k | 49.1k |
| arrays/sort 10000 | 65.70 µs | 364.52 µs | 0.18× | 1 | 1 | 78.2k | 80.0k | 1.37M | 2.42M | 319.2k | 1.73M |
| tensors/matmul 64 | 79.42 µs | 491.23 µs | 0.16× | 2 ⚠ | 1 | 32.1k | 32.0k | 1.57M | 4.30M | 386.8k | 2.44M |
| arrays/push 1000 | 506.5 ns | 3.44 µs | 0.15× | 1 | 12 | 7.8k | 24.6k | 13.0k | 41.7k | 2.4k | 21.5k |
| tensors/broadcast 256 | 10.40 µs | 74.86 µs | 0.14× | 2 ⚠ | 1 | 512.1k | 512.0k | 189.7k | 1.46M | 50.4k | 560.5k |
| floats/dot 100000 | 16.17 µs | 240.40 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 1.00M | 78.7k | 1.20M |
| floats/sum 100000 | 7.87 µs | 239.95 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 601.5k | 38.5k | 1.20M |
| texts/toUpper ascii | 76.9 ns | 2.39 µs | 0.03× | 2 ⚠ | 1 | 2.2k | 1.1k | 1.9k | 51.6k | 369 | 11.3k |

14 of 69 benchmarks are slower than Go; 34 allocate more; 19 run more instructions than Go.

The rows still behind Go, and why:

- `queries/iter filter sumBy`, `queries/numbers iter map filter sum`: one
  loop now, its element in registers and its closures inlined, but each
  turn still steps through the adapters' `next`: the tag of each step's
  option, the position checked against the length and kept from being
  negative, and the sum's check of overflow; Go's loop is a bare `for
  range`.
- `texts/trim`: a trimmed text is a new one holding its bytes, where Go's
  is a view of the old; texts short enough to live in a frame (change 33,
  rule 8) are what would remove it.
- `texts/chars ascii` and `other`: the loop reads an ASCII character in
  place and the text's length once, before it, but checks its count for
  overflow each turn and jumps twice for each ASCII character, out of the
  test and back to the loop's head, where Go's loop jumps once.
- `texts/parse ints`: a call of the runtime for each number, which counted
  in 128 bits; since this table it reads an `Int` in decimal of up to 18
  digits in 64 bits, 21% faster, 6.49 µs against Go's 6.41.
- `queries/countBy`: each element's key takes two calls of the runtime,
  one that finds its entry and one for where the entries start, to count
  its position, where Go's takes one; the hash itself takes a word at a
  time.
- `ffi/*`: within 10% of Go. A thread of C new to the runtime costs it the
  records it keeps for a thread, and each call a task's record in the
  thread's block.
- `tasks/spawn`, 15%: each task's record with its join, below; `texts/toUpper
  other`, `queries/uniqueBy`, `floats/sqrt`: within 10% of Go, as much as a
  row moves from one run to the next.
- `net/tls echo` makes about three more allocations a round trip than Go,
  and `http/tls` a few more a request: rustls 0.23 makes a buffer for each
  record it encrypts and copies each one it reads into another, in its
  buffered API and its unbuffered one alike.

`B/op` counts what each side's allocator gives. In `tasks/spawn` Tondo
allocates each task's record with its join, about 600 bytes, where Go takes
its goroutines' from a list of those that ended. Where Tondo allocates as
many times as Go but more bytes, as `strings/split`, its pieces are texts of
their own, each with a header and its bytes, where Go's are views of the
first. `http/json` makes fewer allocations than Go and 1.7 times the bytes,
about 22 KB a request against Go's 13.

The servers of the `http` rows at 20000 requests a second, each sent when
due whether or not those before came back, its latency from then until its
answer, after half a second that opens the connections (`bench/latency`):

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 55 µs | 394 µs | 76 µs | 1524 µs |
| http/json 20000/s | 88 µs | 202 µs | 133 µs | 1826 µs |
| http/tls hello 20000/s | 110 µs ⚠ | 90 µs | 175 µs | 1587 µs |

Tondo's median is behind Go's on `http/tls hello` alone, and its 99th
percentile is about a tenth of Go's on every server; why the median of
TLS is behind is still to be found.

The rows that LINQ says something of, against C# (`bench/csharp`, measured
as `go test -bench` measures, its bytes from the allocator and no count of
allocations):

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 50.93 µs | 63.06 µs | 0.81× | 40 | 96 |
| queries/numbers iter map filter sum 10000 | 19.21 µs | 24.14 µs | 0.80× | 80 | 128 |
| queries/countBy 10000 | 143.87 µs | 82.37 µs | 1.75× ⚠ | 304 | 1.1k |
| queries/uniqueBy 10000 | 150.82 µs | 61.97 µs | 2.43× ⚠ | 128.1k | 65.2k |
| queries/maxBy 10000 | 6.66 µs | 16.29 µs | 0.41× | 0 | 40 |
| queries/joinBy 10000x1000 | 454.21 µs | 137.32 µs | 3.31× ⚠ | 1.67M | 102.3k |
| queries/groupBy 10000 | 265.86 µs | 275.88 µs | 0.96× | 879.9k | 583.9k |
| queries/filter sumBy 10000 | 67.47 µs | 46.98 µs | 1.44× ⚠ | 512.2k | 39.0k |
| queries/numbers map filter sum 10000 | 16.76 µs | 10.88 µs | 1.54× ⚠ | 142.5k | 51.7k |
| queries/averageBy 10000 | 10.56 µs | 25.66 µs | 0.41× | 0 | 24 |
| queries/toSet 10000 | 194.86 µs | 48.01 µs | 4.06× ⚠ | 1024.0k | 158.0k |
| queries/sortedBy 10000 | 1.53 ms | 521.04 µs | 2.93× ⚠ | 1.45M | 234.7k |
| queries/keyedBy 10000 | 207.04 µs | 73.75 µs | 2.81× ⚠ | 2.00M | 276.4k |

Tondo is ahead of C# on both lazy chains, `maxBy`, `averageBy` and
`groupBy`; C# on the rest, by half on the eager chains and on `countBy`, and
two to four times on sorting, on sets and on most maps. C#'s times move more
than the other sides' from one run to the next.

Both for x86-64-v3, `--cpu x86-64-v3` and `GOAMD64=v3`, with
`compare.to --cpu x86-64-v3`, on 2026-11-15 at 7f4677ad, before the changes
that put `texts/chars` and `queries/countBy` closer to Go:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 49.94 µs | 6.24 µs | 8.01× ⚠ | 1 ⚠ | 0 | 40 | 0 | 550.1k | 80.0k | 242.3k | 30.4k |
| texts/trim | 23.6 ns | 5.2 ns | 4.53× ⚠ | 1 ⚠ | 0 | 67 | 0 | 538 | 115 | 113 | 25 |
| queries/numbers iter map filter sum 10000 | 27.60 µs | 7.12 µs | 3.88× ⚠ | 2 ⚠ | 0 | 80 | 0 | 434.9k | 142.8k | 136.5k | 34.4k |
| texts/chars ascii | 1.15 µs | 306.0 ns | 3.75× ⚠ | 0 | 0 | 0 | 0 | 34.2k | 9.9k | 5.6k | 1.5k |
| texts/parse ints 1000 | 15.64 µs | 6.42 µs | 2.44× ⚠ | 0 | 0 | 0 | 0 | 350.4k | 144.4k | 75.8k | 30.9k |
| queries/countBy 10000 | 217.14 µs | 106.01 µs | 2.05× ⚠ | 6 ⚠ | 2 | 304 | 256 | 2.66M | 1.54M | 1.04M | 511.5k |
| texts/chars other | 2.26 µs | 1.16 µs | 1.95× ⚠ | 0 | 0 | 0 | 0 | 53.6k | 28.2k | 10.9k | 5.6k |
| tasks/spawn 1000 | 189.07 µs | 173.14 µs | 1.09× ⚠ | 1952 ⚠ | 2002 | 634.1k | 78.5k | 2.25M | 2.34M | 1.37M | 1.77M |
| queries/uniqueBy 10000 | 156.04 µs | 145.63 µs | 1.07× ⚠ | 39 ⚠ | 32 | 128.1k | 160.2k | 2.21M | 1.93M | 731.7k | 749.0k |
| texts/toUpper other | 8.65 µs | 8.08 µs | 1.07× ⚠ | 2 ⚠ | 1 | 3.4k | 1.8k | 136.4k | 171.8k | 42.0k | 38.4k |
| loops/collatz 10000 | 900.88 µs | 845.80 µs | 1.07× ⚠ | 0 | 0 | 0 | 0 | 15.37M | 8.86M | 4.40M | 4.12M |
| floats/mandelbrot 64 | 134.29 µs | 127.36 µs | 1.05× ⚠ | 0 | 0 | 0 | 0 | 1.19M | 1.42M | 666.5k | 626.6k |
| ffi/from C threads 8x1000 | 179.89 µs | 173.24 µs | 1.04× ⚠ | 16 ⚠ | 0 | 4.8k | 4 | 12.9k | 4.12M | 33.4k | 1.47M |
| net/tls echo 1x1000 | 12.77 ms | 12.51 ms | 1.02× ⚠ | 7000 ⚠ | 3744 | 172.1k | 50.7k | 18.70M | 14.15M | 14.15M | 11.83M |
| floats/sqrt 100000 | 177.88 µs | 174.98 µs | 1.02× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 699.8k | 890.7k | 876.4k |
| ffi/from a C thread 1000 | 65.05 µs | 64.84 µs | 1.00× ⚠ | 2 ⚠ | 0 | 608 | 0 | 1.8k | 517.0k | 4.4k | 170.6k |
| calls/closure 1000 | 203.9 ns | 207.1 ns | 0.98× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| strings/split 100 | 761.7 ns | 779.0 ns | 0.98× | 1 ⚠ | 1 | 4.4k | 1.8k | 16.0k | 13.8k | 3.7k | 4.0k |
| runtime/mutex 4x1000 | 20.28 µs | 20.96 µs | 0.97× | 6 ⚠ | 6 | 432 | 224 | 408.7k | 120.4k | 94.4k | 146.1k |
| texts/split other | 1.54 µs | 1.59 µs | 0.96× | 1 ⚠ | 1 | 9.4k | 3.4k | 31.8k | 27.1k | 7.4k | 7.7k |
| arrays/index sum 10000 | 2.07 µs | 2.18 µs | 0.95× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.6k |
| net/echo 1x1000 | 11.12 ms | 11.76 ms | 0.95× | 3000 ⚠ | 3744 | 94.4k | 50.5k | 10.66M | 7.10M | 8.71M | 7.57M |
| strings/builder 100 | 755.8 ns | 804.3 ns | 0.94× | 6 | 8 | 1.2k | 1.9k | 16.3k | 13.4k | 3.6k | 4.2k |
| ffi/waits from a C thread 100 | 941.07 µs | 1.05 ms | 0.90× | 109 | 200 | 5.3k | 12.5k | 230.9k | 2.26M | 468.8k | 2.77M |
| json/decode struct | 1.12 µs | 1.29 µs | 0.87× | 2 | 4 | 280 | 288 | 17.9k | 21.0k | 5.4k | 6.1k |
| runtime/join 1 | 266.8 ns | 309.5 ns | 0.86× | 1 | 2 | 40 | 144 | 2.8k | 3.2k | 1.3k | 1.4k |
| texts/join 100 | 523.2 ns | 610.7 ns | 0.86× | 1 ⚠ | 1 | 1.0k | 576 | 8.2k | 11.5k | 2.5k | 2.9k |
| loops/sum 1000000 | 611.05 µs | 728.71 µs | 0.84× | 0 | 0 | 0 | 0 | 8.25M | 15.00M | 3.00M | 3.58M |
| runtime/select 1000 | 121.54 µs | 147.08 µs | 0.83× | 4 ⚠ | 4 | 722 | 528 | 1.32M | 2.09M | 545.6k | 719.6k |
| strings/contains 1000 | 12.9 ns | 15.7 ns | 0.82× | 0 | 0 | 0 | 0 | 332 | 394 | 62 | 76 |
| http/hello 1x1000 | 32.96 ms | 40.72 ms | 0.81× | 62000 ⚠ | 69155 | 19.90M | 6.19M | 70.48M | 147.21M | 87.29M | 140.20M |
| queries/maxBy 10000 | 6.59 µs | 8.37 µs | 0.79× | 0 | 0 | 0 | 0 | 145.3k | 150.1k | 31.9k | 39.8k |
| maps/lookup 10000 ints | 54.72 µs | 70.20 µs | 0.78× | 0 | 0 | 0 | 0 | 970.1k | 1.01M | 261.6k | 332.0k |
| calls/fib 25 | 216.81 µs | 290.62 µs | 0.75× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.06M | 1.40M |
| json/encode 100 structs | 22.88 µs | 30.77 µs | 0.74× | 2 | 4 | 7.2k | 16.1k | 404.3k | 441.1k | 109.6k | 145.7k |
| http/tls 64x100 | 65.67 ms | 89.82 ms | 0.73× | 462083 ⚠ | 460360 | 54.67M | 51.55M | 606.05M | 1.75G | 929.35M | 1.65G |
| strings/interpolate | 23.1 ns | 33.3 ns | 0.69× | 1 ⚠ | 1 | 52 | 32 | 495 | 663 | 110 | 158 |
| tasks/channel 10000 | 192.91 µs | 283.95 µs | 0.68× | 2 ⚠ | 2 | 745 | 664 | 3.16M | 4.42M | 904.4k | 1.34M |
| strings/int to text 1000 | 7.91 µs | 11.73 µs | 0.67× | 0 | 900 | 0 | 2.8k | 163.6k | 211.4k | 38.0k | 55.9k |
| queries/joinBy 10000x1000 | 461.27 µs | 760.03 µs | 0.61× | 1045 ⚠ | 1039 | 1.67M | 2.91M | 6.38M | 12.93M | 2.20M | 6.04M |
| json/parse object | 2.27 µs | 3.82 µs | 0.59× | 17 ⚠ | 36 | 2.3k | 1.5k | 35.0k | 47.9k | 10.9k | 17.8k |
| json/parse 1000 numbers | 77.42 µs | 132.28 µs | 0.59× | 909 ⚠ | 2015 | 55.9k | 54.2k | 1.38M | 2.11M | 375.4k | 619.0k |
| texts/replace ascii | 680.7 ns | 1.26 µs | 0.54× | 9 ⚠ | 1 | 4.6k | 1.0k | 14.3k | 24.6k | 3.2k | 5.9k |
| queries/groupBy 10000 | 274.35 µs | 531.47 µs | 0.52× | 2683 | 4664 | 879.9k | 1.18M | 3.51M | 7.59M | 1.28M | 3.69M |
| arrays/map filter sum 10000 | 16.97 µs | 33.05 µs | 0.51× | 12 | 16 | 142.5k | 165.2k | 374.3k | 485.8k | 81.1k | 188.9k |
| queries/filter sumBy 10000 | 67.99 µs | 140.32 µs | 0.48× | 12 | 16 | 512.2k | 727.7k | 822.2k | 2.47M | 328.2k | 1.22M |
| structs/users 1000 | 18.73 µs | 39.73 µs | 0.47× | 1001 | 1910 | 48.3k | 72.8k | 404.5k | 669.2k | 89.5k | 187.2k |
| queries/numbers map filter sum 10000 | 16.77 µs | 36.37 µs | 0.46× | 12 | 16 | 142.5k | 165.2k | 374.2k | 664.8k | 80.3k | 271.9k |
| texts/replace other | 528.0 ns | 1.18 µs | 0.45× | 3 ⚠ | 1 | 6.8k | 1.8k | 11.9k | 19.6k | 2.5k | 5.7k |
| queries/averageBy 10000 | 10.72 µs | 24.14 µs | 0.44× | 0 | 0 | 0 | 0 | 200.2k | 90.1k | 51.3k | 121.8k |
| maps/insert 1000 strings | 43.69 µs | 102.68 µs | 0.43× | 1030 | 1920 | 111.6k | 121.0k | 675.5k | 1.30M | 209.7k | 472.6k |
| queries/toSet 10000 | 197.30 µs | 487.88 µs | 0.40× | 42 ⚠ | 81 | 1024.0k | 577.8k | 2.42M | 5.87M | 934.9k | 2.59M |
| queries/sortedBy 10000 | 1.52 ms | 3.91 ms | 0.39× | 4 ⚠ | 2 | 1.45M | 392.0k | 18.95M | 31.78M | 7.27M | 19.37M |
| texts/contains other | 177.9 ns | 480.0 ns | 0.37× | 0 | 0 | 0 | 0 | 3.5k | 8.2k | 860 | 2.3k |
| ffi/callback 1000 | 10.41 µs | 29.94 µs | 0.35× | 0 | 0 | 0 | 0 | 140.2k | 497.4k | 47.2k | 175.8k |
| http/json 64x100 | 31.20 ms | 100.26 ms | 0.31× | 556989 ⚠ | 710796 | 141.27M | 114.13M | 676.58M | 3.74G | 1.82G | 6.61G |
| queries/keyedBy 10000 | 209.78 µs | 690.69 µs | 0.30× | 42 ⚠ | 81 | 2.00M | 1.75M | 3.22M | 9.24M | 992.5k | 4.28M |
| maps/insert 10000 ints | 136.15 µs | 495.74 µs | 0.27× | 42 ⚠ | 79 | 1.25M | 577.6k | 2.33M | 5.31M | 654.4k | 2.33M |
| regex/match 1000 | 38.05 µs | 154.40 µs | 0.25× | 0 | 0 | 0 | 0 | 571.3k | 2.99M | 184.7k | 745.4k |
| http/hello 64x100 | 19.22 ms | 98.28 ms | 0.20× | 397277 ⚠ | 453075 | 127.38M | 45.42M | 448.82M | 3.18G | 1.14G | 6.53G |
| floats/add 100000 | 18.24 µs | 96.22 µs | 0.19× | 1 | 1 | 781.3k | 784.0k | 281.3k | 1.70M | 87.7k | 603.3k |
| arrays/sort 10000 | 66.59 µs | 364.79 µs | 0.18× | 1 | 1 | 78.2k | 80.0k | 1.37M | 2.42M | 320.7k | 1.75M |
| structs/points 1000 | 1.33 µs | 7.62 µs | 0.18× | 1 | 12 | 15.7k | 49.2k | 30.7k | 79.7k | 6.4k | 44.7k |
| tensors/matmul 64 | 80.05 µs | 492.72 µs | 0.16× | 2 ⚠ | 1 | 32.1k | 32.0k | 1.57M | 4.30M | 387.3k | 2.41M |
| tensors/broadcast 256 | 10.21 µs | 70.93 µs | 0.14× | 2 ⚠ | 1 | 512.1k | 512.0k | 189.7k | 1.45M | 50.5k | 526.6k |
| arrays/push 1000 | 513.8 ns | 3.58 µs | 0.14× | 1 | 12 | 7.8k | 24.6k | 13.1k | 42.5k | 2.5k | 24.9k |
| floats/dot 100000 | 16.33 µs | 242.72 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 1.00M | 78.6k | 1.20M |
| floats/sum 100000 | 7.95 µs | 241.05 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 600.0k | 38.4k | 1.20M |
| texts/toUpper ascii | 76.8 ns | 2.38 µs | 0.03× | 2 ⚠ | 1 | 2.2k | 1.1k | 1.9k | 51.7k | 367 | 11.4k |

16 of 69 benchmarks are slower than Go; 34 allocate more; 19 run more instructions than Go.

The servers at this level:

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 57 µs | 286 µs | 77 µs | 1412 µs |
| http/json 20000/s | 78 µs | 203 µs | 132 µs | 1428 µs |
| http/tls hello 20000/s | 95 µs ⚠ | 89 µs | 147 µs | 1512 µs |

The same rows are behind at this level, but `strings/split`, and one more:
Go makes more of FMA in `floats/mandelbrot`. `loops/collatz`, whose code is
the same at both levels, is where its code lies, from 736 µs to 871 µs over
four orders of the functions at `baseline` when last measured so.

What the level changes of Tondo's own code, measured on 2026-11-14 at
e66c13bf, every benchmark at `baseline` and at `x86-64-v3`, by the instructions and cycles of a run, the fewest
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
