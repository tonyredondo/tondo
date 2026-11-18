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

The rows of `programs/` are small programs, each a shape programs have,
that combine what the other rows measure alone, so that a change is judged
by how they go together too: a text split and counted into a map; lines
read with `?`, parsed and summed; records filtered, sorted and joined into
a text; an order decoded from JSON, changed and encoded again; and a tree
built, summed and let go of.

The rows of `ffi/` call each side from C: a loop of C that calls a function
given to it, from the benchmark's own thread, which in Tondo is a task's,
and from threads that C starts and the runtime has never seen, and a call
that waits once, for a task or a goroutine it starts. Each side compiles
the same `callers.c` with the system's C compiler, Go's through cgo; without
one, Tondo's test skips them and Go builds without them.

This table was measured on an AMD Ryzen 9 5950X, Linux, with Go 1.27, on
2026-11-18 at 9e7b747b, each side for its default level of processor:
Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`, as the tables of
latency and of C# below it. A row is marked when it is slower than Go, and
its allocations when it allocates more, in number or in bytes, on the
heap: the columns of allocations and bytes are what each side's allocator
gives, which compare with Go's, and beside Tondo's, what it made in
regions besides, which Go would give its heap where the value leaves its
function:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| texts/trim | 14.9 ns | 5.2 ns | 2.88× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 355 | 115 | 72 | 25 |
| programs/sum lines | 38.79 µs | 15.61 µs | 2.48× ⚠ | 5 ⚠ +1001 in regions | 2 | 44.8k +31.4k in regions | 4.0k | 724.8k | 299.3k | 189.4k | 74.9k |
| queries/numbers iter map filter sum 10000 | 16.87 µs | 7.08 µs | 2.38× ⚠ | 0 +2 in regions | 0 | 0 +96 in regions | 0 | 414.7k | 142.8k | 81.7k | 34.6k |
| texts/chars ascii | 677.6 ns | 307.1 ns | 2.21× ⚠ | 0 | 0 | 0 | 0 | 11.1k | 9.9k | 3.3k | 1.5k |
| queries/iter filter sumBy 10000 | 11.48 µs | 6.18 µs | 1.86× ⚠ | 0 +1 in regions | 0 | 0 +48 in regions | 0 | 249.9k | 80.0k | 55.4k | 30.0k |
| texts/chars other | 1.74 µs | 1.14 µs | 1.53× ⚠ | 0 | 0 | 0 | 0 | 33.4k | 28.2k | 8.5k | 6.0k |
| queries/countBy 10000 | 152.73 µs | 110.54 µs | 1.38× ⚠ | 0 +6 in regions | 2 | 0 +304 in regions | 256 | 2.54M | 1.55M | 735.3k | 532.3k |
| programs/count words | 14.13 µs | 11.15 µs | 1.27× ⚠ | 0 +7 in regions | 3 | 0 +25.1k in regions | 9.8k | 255.3k | 185.2k | 67.9k | 51.3k |
| json/decode struct | 1.44 µs | 1.24 µs | 1.16× ⚠ | 2 | 4 | 280 | 288 | 18.1k | 21.1k | 7.1k | 6.1k |
| ffi/from a C thread 1000 | 66.12 µs | 60.43 µs | 1.09× ⚠ | 2 ⚠ | 0 | 625 | 0 | 1.8k | 516.1k | 4.3k | 169.7k |
| texts/toUpper other | 8.67 µs | 7.98 µs | 1.09× ⚠ | 1 +1 in regions | 1 | 1.7k +1.7k in regions | 1.8k | 136.3k | 172.4k | 42.3k | 37.8k |
| tasks/spawn 1000 | 182.11 µs | 170.31 µs | 1.07× ⚠ | 1953 ⚠ | 2002 | 648.3k | 78.5k | 2.22M | 2.28M | 1.36M | 1.62M |
| ffi/from C threads 8x1000 | 181.55 µs | 172.60 µs | 1.05× ⚠ | 16 ⚠ | 0 | 4.9k | 4 | 13.0k | 4.12M | 34.7k | 1.49M |
| texts/parse ints 1000 | 6.67 µs | 6.39 µs | 1.04× ⚠ | 0 | 0 | 0 | 0 | 160.5k | 144.4k | 32.3k | 30.8k |
| net/tls echo 1x1000 | 12.71 ms | 12.19 ms | 1.04× ⚠ | 5000 ⚠ +2000 in regions | 3744 | 108.8k +78.1k in regions | 50.7k | 18.51M | 14.30M | 14.63M | 11.80M |
| loops/collatz 10000 | 862.74 µs | 840.39 µs | 1.03× ⚠ | 0 | 0 | 0 | 0 | 15.37M | 8.86M | 4.24M | 4.12M |
| runtime/mutex 4x1000 | 21.24 µs | 20.75 µs | 1.02× ⚠ | 5 ⚠ +1 in regions | 6 | 401 +32 in regions | 224 | 408.7k | 121.4k | 99.5k | 147.7k |
| floats/sqrt 100000 | 176.02 µs | 174.36 µs | 1.01× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.0k | 888.6k | 878.2k |
| queries/uniqueBy 10000 | 148.98 µs | 148.73 µs | 1.00× ⚠ | 0 +39 in regions | 32 | 0 +128.2k in regions | 160.2k | 2.22M | 1.96M | 716.8k | 770.3k |
| net/echo 1x1000 | 11.14 ms | 11.33 ms | 0.98× | 1000 +2000 in regions | 3744 | 31.1k +78.1k in regions | 50.5k | 10.47M | 7.16M | 8.90M | 7.61M |
| floats/mandelbrot 64 | 135.15 µs | 138.30 µs | 0.98× | 0 | 0 | 0 | 0 | 1.48M | 1.57M | 671.9k | 687.0k |
| arrays/index sum 10000 | 2.07 µs | 2.16 µs | 0.96× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.3k |
| programs/sort adults | 238.74 µs | 268.46 µs | 0.89× | 13 ⚠ +697 in regions | 710 | 92.0k +58.1k in regions | 90.4k | 2.55M | 4.05M | 1.09M | 1.25M |
| strings/builder 100 | 710.5 ns | 807.0 ns | 0.88× | 0 +6 in regions | 8 | 0 +1.2k in regions | 1.9k | 15.3k | 13.6k | 3.4k | 4.0k |
| runtime/select 1000 | 123.55 µs | 143.90 µs | 0.86× | 4 ⚠ | 4 | 723 | 529 | 1.33M | 2.09M | 546.7k | 752.6k |
| runtime/join 1 | 263.3 ns | 313.7 ns | 0.84× | 1 | 2 | 40 | 144 | 2.8k | 3.3k | 1.2k | 1.5k |
| strings/contains 1000 | 13.1 ns | 15.7 ns | 0.83× | 0 | 0 | 0 | 0 | 333 | 394 | 63 | 76 |
| loops/sum 1000000 | 606.91 µs | 731.74 µs | 0.83× | 0 | 0 | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.57M |
| http/hello 1x1000 | 32.37 ms | 40.28 ms | 0.80× | 51000 ⚠ +1000 in regions | 69147 | 19.51M +109.4k in regions | 6.18M | 67.64M | 147.77M | 81.03M | 144.53M |
| queries/maxBy 10000 | 6.67 µs | 8.33 µs | 0.80× | 0 | 0 | 0 | 0 | 145.3k | 150.0k | 32.6k | 40.2k |
| strings/split 100 | 618.7 ns | 775.2 ns | 0.80× | 0 +1 in regions | 1 | 0 +4.4k in regions | 1.8k | 13.5k | 13.9k | 3.0k | 4.0k |
| calls/fib 25 | 230.64 µs | 289.05 µs | 0.80× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.13M | 1.40M |
| texts/split other | 1.26 µs | 1.59 µs | 0.80× | 0 +1 in regions | 1 | 0 +9.4k in regions | 3.4k | 27.1k | 27.1k | 6.1k | 7.8k |
| programs/reprice order | 1.31 µs | 1.67 µs | 0.78× | 0 +7 in regions | 9 | 0 +352 in regions | 627 | 18.9k | 20.9k | 6.4k | 7.8k |
| json/encode 100 structs | 22.78 µs | 30.35 µs | 0.75× | 1 +1 in regions | 4 | 1 +7.2k in regions | 16.1k | 404.8k | 441.6k | 109.9k | 151.7k |
| maps/lookup 10000 ints | 54.31 µs | 74.31 µs | 0.73× | 0 | 0 | 0 | 0 | 970.1k | 1.06M | 258.6k | 357.1k |
| texts/join 100 | 474.3 ns | 653.1 ns | 0.73× | 0 +1 in regions | 1 | 0 +1.0k in regions | 576 | 8.1k | 11.5k | 2.3k | 3.1k |
| tasks/channel 10000 | 192.30 µs | 268.24 µs | 0.72× | 2 ⚠ | 2 | 744 | 664 | 3.17M | 4.41M | 922.7k | 1.27M |
| strings/int to text 1000 | 7.69 µs | 11.23 µs | 0.68× | 0 | 900 | 0 | 2.8k | 163.6k | 211.6k | 37.2k | 54.1k |
| http/tls 64x100 | 56.93 ms | 84.33 ms | 0.68× | 391952 ⚠ +6405 in regions | 459729 | 52.11M +701.1k in regions | 51.39M | 585.64M | 1.73G | 817.54M | 1.48G |
| queries/joinBy 10000x1000 | 467.22 µs | 697.93 µs | 0.67× | 1045 ⚠ | 1039 | 1.67M | 2.91M | 6.28M | 13.43M | 2.26M | 6.20M |
| ffi/waits from a C thread 100 | 714.19 µs | 1.07 ms | 0.67× | 109 | 200 | 5.3k | 12.5k | 228.7k | 1.98M | 335.0k | 1.92M |
| json/parse object | 2.42 µs | 3.79 µs | 0.64× | 17 ⚠ | 36 | 2.3k | 1.5k | 35.1k | 48.4k | 11.7k | 18.1k |
| json/parse 1000 numbers | 86.54 µs | 140.85 µs | 0.61× | 909 ⚠ | 2015 | 55.9k | 54.2k | 1.40M | 2.12M | 398.3k | 659.6k |
| strings/interpolate | 19.3 ns | 33.2 ns | 0.58× | 0 +1 in regions | 1 | 0 +64 in regions | 32 | 410 | 666 | 93 | 159 |
| queries/groupBy 10000 | 266.48 µs | 527.42 µs | 0.51× | 2683 | 4664 | 879.9k | 1.18M | 3.56M | 7.82M | 1.28M | 3.76M |
| calls/closure 1000 | 203.8 ns | 411.6 ns | 0.50× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 2.0k |
| arrays/map filter sum 10000 | 16.91 µs | 35.38 µs | 0.48× | 0 +12 in regions | 16 | 0 +142.5k in regions | 165.2k | 373.6k | 486.2k | 81.0k | 199.0k |
| queries/numbers map filter sum 10000 | 16.57 µs | 36.51 µs | 0.45× | 0 +12 in regions | 16 | 0 +142.5k in regions | 165.2k | 373.6k | 662.1k | 79.8k | 278.5k |
| texts/replace other | 494.8 ns | 1.11 µs | 0.45× | 0 +1 in regions | 1 | 0 +1.7k in regions | 1.8k | 11.5k | 19.4k | 2.4k | 5.3k |
| texts/replace ascii | 555.5 ns | 1.26 µs | 0.44× | 0 +1 in regions | 1 | 0 +1.0k in regions | 1.0k | 12.9k | 24.6k | 2.8k | 5.9k |
| queries/averageBy 10000 | 10.64 µs | 24.20 µs | 0.44× | 0 | 0 | 0 | 0 | 200.2k | 90.1k | 51.4k | 118.9k |
| queries/toSet 10000 | 200.14 µs | 491.68 µs | 0.41× | 0 +42 in regions | 81 | 0 +1.00M in regions | 577.8k | 2.41M | 5.86M | 976.8k | 2.59M |
| queries/filter sumBy 10000 | 55.55 µs | 138.39 µs | 0.40× | 0 +12 in regions | 16 | 0 +512.2k in regions | 727.7k | 673.2k | 2.48M | 269.9k | 1.21M |
| queries/sortedBy 10000 | 1.46 ms | 3.90 ms | 0.37× | 4 ⚠ | 2 | 1.45M | 392.0k | 18.95M | 31.64M | 7.06M | 19.47M |
| programs/build tree | 49.44 µs | 133.20 µs | 0.37× | 0 +1365 in regions | 4095 | 0 +128.0k in regions | 298.6k | 981.7k | 2.21M | 238.2k | 670.7k |
| texts/contains other | 171.8 ns | 471.1 ns | 0.36× | 0 | 0 | 0 | 0 | 3.5k | 8.2k | 830 | 2.3k |
| http/json 64x100 | 30.78 ms | 88.28 ms | 0.35× | 486558 ⚠ +12805 in regions | 704335 | 138.21M +801.1k in regions | 100.62M | 657.05M | 3.57G | 1.76G | 6.29G |
| ffi/callback 1000 | 10.06 µs | 30.01 µs | 0.34× | 0 | 0 | 0 | 0 | 140.2k | 497.4k | 48.2k | 144.4k |
| structs/users 1000 | 11.68 µs | 39.87 µs | 0.29× | 0 +1001 in regions | 1910 | 0 +62.5k in regions | 72.8k | 249.8k | 672.2k | 56.2k | 176.5k |
| maps/insert 10000 ints | 143.21 µs | 500.43 µs | 0.29× | 0 +42 in regions | 79 | 0 +1.25M in regions | 577.6k | 2.31M | 5.37M | 663.2k | 2.38M |
| maps/insert 1000 strings | 28.38 µs | 103.58 µs | 0.27× | 0 +1030 in regions | 1920 | 0 +126.9k in regions | 121.0k | 499.4k | 1.30M | 136.8k | 473.4k |
| queries/keyedBy 10000 | 184.91 µs | 695.73 µs | 0.27× | 0 +42 in regions | 81 | 0 +2.00M in regions | 1.75M | 2.95M | 9.36M | 870.4k | 4.28M |
| regex/match 1000 | 37.81 µs | 152.53 µs | 0.25× | 0 | 0 | 0 | 0 | 579.8k | 2.99M | 183.7k | 736.2k |
| http/hello 64x100 | 19.63 ms | 80.62 ms | 0.24× | 326886 ⚠ +6405 in regions | 448670 | 124.85M +701.1k in regions | 43.92M | 429.45M | 3.29G | 1.07G | 7.92G |
| floats/add 100000 | 17.83 µs | 81.52 µs | 0.22× | 1 | 1 | 781.3k | 784.0k | 280.9k | 1.71M | 86.0k | 554.7k |
| arrays/sort 10000 | 63.85 µs | 361.12 µs | 0.18× | 0 +1 in regions | 1 | 0 +78.2k in regions | 80.0k | 1.32M | 2.41M | 309.3k | 1.72M |
| structs/points 1000 | 1.31 µs | 7.67 µs | 0.17× | 0 +1 in regions | 12 | 0 +15.7k in regions | 49.2k | 30.3k | 85.6k | 6.4k | 42.2k |
| tensors/matmul 64 | 79.82 µs | 489.69 µs | 0.16× | 2 ⚠ | 1 | 32.1k | 32.0k | 1.57M | 4.30M | 395.4k | 2.45M |
| tensors/broadcast 256 | 11.45 µs | 70.73 µs | 0.16× | 1 ⚠ +1 in regions | 1 | 512.0k +48 in regions | 512.0k | 189.8k | 1.45M | 48.6k | 495.5k |
| arrays/push 1000 | 517.4 ns | 3.59 µs | 0.14× | 0 +1 in regions | 12 | 0 +7.8k in regions | 24.6k | 12.8k | 41.4k | 2.5k | 22.7k |
| floats/dot 100000 | 16.20 µs | 240.33 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 1.00M | 79.0k | 1.21M |
| floats/sum 100000 | 7.84 µs | 240.19 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 600.2k | 38.3k | 1.20M |
| texts/toUpper ascii | 56.4 ns | 2.38 µs | 0.02× | 0 +1 in regions | 1 | 0 +1.1k in regions | 1.1k | 1.4k | 51.6k | 272 | 11.4k |

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 54 µs | 395 µs | 79 µs | 1538 µs |
| http/json 20000/s | 104 µs | 203 µs | 158 µs | 1761 µs |
| http/tls hello 20000/s | 91 µs ⚠ | 89 µs | 136 µs | 1572 µs |

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/numbers iter map filter sum 10000 | 16.87 µs | 24.29 µs | 0.69× | 0 | 128 |
| queries/iter filter sumBy 10000 | 11.48 µs | 67.84 µs | 0.17× | 0 | 96 |
| queries/countBy 10000 | 152.73 µs | 83.02 µs | 1.84× ⚠ | 0 | 1.1k |
| queries/uniqueBy 10000 | 148.98 µs | 61.57 µs | 2.42× ⚠ | 0 | 65.2k |
| queries/maxBy 10000 | 6.67 µs | 16.57 µs | 0.40× | 0 | 40 |
| queries/joinBy 10000x1000 | 467.22 µs | 134.60 µs | 3.47× ⚠ | 1.67M | 102.3k |
| queries/groupBy 10000 | 266.48 µs | 275.07 µs | 0.97× | 879.9k | 583.9k |
| queries/numbers map filter sum 10000 | 16.57 µs | 10.39 µs | 1.59× ⚠ | 0 | 51.7k |
| queries/averageBy 10000 | 10.64 µs | 25.55 µs | 0.42× | 0 | 24 |
| queries/toSet 10000 | 200.14 µs | 50.35 µs | 3.97× ⚠ | 0 | 158.0k |
| queries/filter sumBy 10000 | 55.55 µs | 48.27 µs | 1.15× ⚠ | 0 | 39.0k |
| queries/sortedBy 10000 | 1.46 ms | 516.36 µs | 2.82× ⚠ | 1.45M | 234.7k |
| queries/keyedBy 10000 | 184.91 µs | 73.37 µs | 2.52× ⚠ | 0 | 276.4k |

19 of 74 benchmarks are slower than Go; 19 allocate more; 19 run more instructions than Go.

The rows still behind Go, and why:

- `queries/iter filter sumBy`, `queries/numbers iter map filter sum`: one
  loop now, its element in registers and its closures inlined, but each
  turn still steps through the adapters' `next`: the tag of each step's
  option, the position checked against the length and kept from being
  negative, and the sum's check of overflow; Go's loop is a bare `for
  range`.
  They allocate nothing on the heap since their closures go in a region
  (242f1338), and run 249.9 and 414.7 thousand instructions an operation,
  three times Go's: the time is in those. `queries/iter filter sumBy` has
  taken 10.5 µs (80fc037a) and 12.5 µs (448c06a7) with the same
  instructions: where its code lies moves it by a fifth.
- `texts/trim`, 2.9×, from 4.1×: trim reads ASCII a byte at a time and
  gives a text with nothing to trim back as it is (a2d299d7), 355
  instructions where it ran 492, against Go's 115. A trimmed text is still
  a new one holding its bytes, where Go's is a view of the old: a region
  takes it, which costs the heap nothing, but the call and the copy remain,
  and views of texts, lent where they do not outlive their text, are one of
  the two ways of texts before Tony.
- `texts/chars ascii` and `other`: the loop reads an ASCII character in
  place and the text's length once, before it, but checks its count for
  overflow each turn and jumps twice for each ASCII character, out of the
  test and back to the loop's head, where Go's loop jumps once.
  At fccc97c3 the ASCII row takes 681 ns, against 459 ns at 80fc037a,
  with the same instructions and the same code of the benchmark's
  functions (`tondo asm --compiler`): only its cycles grew. A build with
  `--no-lifetimes` takes 461 ns, the text at other places in memory changes
  nothing, and the loop alone, out of the benchmarks' executable, runs as
  fast with either compiler: where the loop's code lies is the likely
  cause, not yet shown.
- `queries/countBy`: each element's key takes two calls of the runtime,
  one that finds its entry and one for where the entries start, to count
  its position, where Go's takes one; the hash itself takes a word at a
  time.
- `programs/sum lines`, 2.5×, and `programs/count words`, 1.3×: behind in
  instructions, 724.8 thousand against 300.7 and 255.4 against 184.7. Each
  line `readLine` gives is a text of its own, a thousand of them in the
  regions of the loop's turns, where Go's scanner gives a view of its
  buffer; and each word of `count words` is a text of its own counted in a
  map. Where the rest goes is for the tightening of regions and the
  optimizer, with `tondo optimizations` and `tondo profile` on each.
- `ffi/*`: within 9% of Go. A thread of C new to the runtime costs it the
  records it keeps for a thread, and each call a task's record in the
  thread's block.
- `loops/collatz`, 3%: 15.4 million instructions against Go's 8.9, in
  about the cycles of Go's, 4.24 million against 4.12.
- `tasks/spawn`, 7%: each task's record with its join, below; and
  `texts/toUpper other`, 9%, `texts/parse ints` and `net/tls echo`, 4%,
  `runtime/mutex`, 2%, `floats/sqrt`, 1%, and `queries/uniqueBy`, at
  1.00×, which the table before had at or under Go's time with Go's side
  measured busy, below. `json/decode struct`, 1.16×, too; its cause is not
  looked at yet. `queries/countBy` is at 1.38×, above.
- The table before this one, at f3c26634, gave 62 rows at or under Go's
  time, and three runs at 9e7b747b give 55, the same rows each time. On
  26 rows of that table Go's time was 12% to 68% over what it is here,
  with the instructions and cycles it has here: its cycles a second were
  as low as 2.4 GHz where it runs at 4.8 here, so its time held time off
  the processor, as when the machine was busy, and seven rows looked under
  Go that are not. `compare.to` now measures a row
  again where one side runs under 80% of the machine's cycles a second
  and the other at them, and marks it ⌛ if it stays so; none did here.
- Go's times of `calls/closure` and `strings/split` are twice those of the
  table before, with the same instructions and no more cycles: a run's
  noise on Go's side, which leaves the two under Go either way.
- `net/tls echo` makes about one more allocation a round trip than Go,
  and `http/tls` a few more a request: rustls 0.23 makes a buffer for each
  record it encrypts and copies each one it reads into another, in its
  buffered API and its unbuffered one alike.

`B/op` counts what each side's allocator gives. In `tasks/spawn` Tondo
allocates each task's record with its join, about 600 bytes, where Go takes
its goroutines' from a list of those that ended. Where Tondo allocates as
many times as Go but more bytes, as `strings/split` in its region, its
pieces are texts of their own, each with a header and its bytes, where Go's
are views of the first. `http/json` makes fewer allocations than Go and 1.5
times the bytes, about 22 KB a request against Go's 15.

The servers of the `http` rows at 20000 requests a second, each sent when
due whether or not those before came back, its latency from then until its
answer, after half a second that opens the connections (`bench/latency`):

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 55 µs | 388 µs | 77 µs | 1594 µs |
| http/json 20000/s | 81 µs | 240 µs | 108 µs | 1676 µs |
| http/tls hello 20000/s | 109 µs ⚠ | 88 µs | 174 µs | 1574 µs |

Tondo's median is behind Go's on `http/tls hello` alone, and its 99th
percentile is about a tenth of Go's on every server; why the median of
TLS is behind is still to be found.

The rows that LINQ says something of, against C# (`bench/csharp`, measured
as `go test -bench` measures, its bytes from the allocator and no count of
allocations):

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/numbers iter map filter sum 10000 | 19.31 µs | 25.63 µs | 0.75× | 80 | 128 |
| queries/iter filter sumBy 10000 | 12.51 µs | 76.53 µs | 0.16× | 40 | 96 |
| queries/countBy 10000 | 142.39 µs | 78.75 µs | 1.81× ⚠ | 304 | 1.1k |
| queries/uniqueBy 10000 | 149.48 µs | 61.25 µs | 2.44× ⚠ | 128.1k | 65.2k |
| queries/maxBy 10000 | 6.70 µs | 16.81 µs | 0.40× | 0 | 40 |
| queries/joinBy 10000x1000 | 445.81 µs | 134.98 µs | 3.30× ⚠ | 1.67M | 102.3k |
| queries/groupBy 10000 | 265.27 µs | 276.88 µs | 0.96× | 879.9k | 583.9k |
| queries/filter sumBy 10000 | 66.18 µs | 45.54 µs | 1.45× ⚠ | 512.2k | 39.0k |
| queries/numbers map filter sum 10000 | 16.67 µs | 10.70 µs | 1.56× ⚠ | 0 | 51.7k |
| queries/averageBy 10000 | 10.45 µs | 25.64 µs | 0.41× | 0 | 24 |
| queries/toSet 10000 | 195.52 µs | 49.59 µs | 3.94× ⚠ | 1024.0k | 158.0k |
| queries/sortedBy 10000 | 1.50 ms | 506.67 µs | 2.95× ⚠ | 1.45M | 234.7k |
| queries/keyedBy 10000 | 211.15 µs | 72.79 µs | 2.90× ⚠ | 2.00M | 276.4k |

Tondo is ahead of C# on both lazy chains, six times on the one over
records, on `maxBy`, `averageBy` and `groupBy`; C# on the rest, 1.4 to 1.6
times on the eager chains, 1.8 times on `countBy`, and two to four times on
sorting, on sets and on most maps. C#'s times move more
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
