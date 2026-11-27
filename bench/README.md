# Tondo against Go

The same work in Tondo and in Go, measured side by side: `tondo/` is a
project whose benchmarks are its tests (`testing.bench`), `go/` a module
whose benchmarks have the same names (`BenchmarkCalls/fib_25` is the
benchmark `fib 25` of the test `calls`), each kept from the optimizer as
the other is. `compare.to`, a Tondo program, runs both and writes the table:

```sh
tondo run bench/compare.to -- --time 1 --output bench/results.md
```

It says first which `go` it runs, and refuses one that cannot build
`go/`, as a system's `go` ahead of the one its `GOROOT` holds on `PATH`,
saying how to put the right one first.

Both sides run in an environment that compare.to sets itself, whatever
started it, and says in its first line: 55 variables, or as many as
`--variables` names. `HOME` as it was given it; `PATH`, the directory of
the `go` it finds, then `/usr/bin` and `/bin`, then those of `dotnet` and
`perf` where they lie elsewhere; `TONDO`, the `tondo` it measures; and
`TABLE_01` on, each `abcdefghijklmnopqrstuvwxyz0123`. The rows that start
a program move with their number and with the directories of `PATH`: Go
copies the environment at each start, and Tondo gives a child its
program's copy once the program has changed its environment. Go runs each
row five times, as its time moves more than Tondo's from one run to the
next where a row allocates much: the table gives their median, and the
fastest and the slowest beside a row whose runs spread past a tenth.

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

`bench/baselines/table.json` holds what `bench/tondo` measured with the
release tondo of this table's commit, on its machine and in its
environment, with the instructions of each row: a change is judged
against it as well as against the commit before it, `--baseline
bench/baselines/table.json`, so that costs too small for one change to
show are seen when they add up.
It is saved again only with a new table, in that table's commit, never
with another change, which would make it follow the drift it is there to
catch; measured on another machine, it compares nothing.

A change to the compiler is measured against the `tondo` of its base,
`--compiler <revision>`, which `tondo compiler` builds once and keeps with
the record of its build: the revision, the sha256 of the executable, and
that cargo built it. Each use checks the record against the bytes, and an
entry without one builds again. A comparison says each side's revision and
sha256 first, and stops when both are one executable, which measures
nothing, unless `--same`: two revisions that differ outside `crates/`,
`vendor/` and `Cargo.*` build the same `tondo`. A measure that runs for
hours goes so:

- Its script runs first on one row of each phase, and the long run starts
  only once every phase wrote its result and ended well.
- A commit measured alone against its parent has a `tondo` of its own when
  it changes `crates/`, `vendor/` or `Cargo.*`: the two sha256s differ, or
  the measure is void before it runs. `tondo asm --all --compiler <parent>`
  over `bench/tondo` then names the functions it changed; "the same code"
  with two executables is a change of where code lies or of the runtime.
- Each result starts with both sides' revisions and sha256s.
- A range of commits is measured with `tondo test <projects> --bench
  --series <base>..<head>`: the `tondo` of each commit that changes the
  compiler, each step against the one before by its instructions,
  allocations and bytes, which repeat, and the whole in turns, with
  `--layouts`. Each measure is kept under what decides it, the `tondo`'s
  sha256, the sources, the options, `TONDO_*` and the machine, so a range
  measured again measures only what is new.

This table was measured on an AMD Ryzen 9 5950X, Linux, with go1.27.4 (mise's,
first on `PATH`), on 2026-11-27 at 3fd8917c, in the environment of 55
variables that compare.to sets, each side for its default level of
processor: Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`, as the
tables of latency and of C# below it. Go's time is the median of its five
runs of each row, and a row whose runs spread past a tenth gives their
fastest and slowest beside it. The rows of a million and of mebibytes run
on the table's one second, as the others: the slowest, `maps/lookup
1000000 ints` at 40 ms a run, still takes some 24 samples, within 0.9%.
A row is marked when it is slower than Go, and its allocations when it
allocates more, in number or in bytes, on the heap: the columns of
allocations and bytes are what each side's allocator gives, which compare
with Go's, and beside Tondo's, what it made in regions besides, which Go
would give its heap where the value leaves its function:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| net/echo 1MiB | 2.84 ms | 243.23 µs | 11.66× ⚠ | 54 ⚠ +22 in regions | 0 | 5.56M +3.56M in regions | 0 | 33.72M | 3.2k | 11.71M | 6.2k |
| texts/trim | 13.8 ns | 4.8 ns | 2.87× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 326 | 115 | 66 | 23 |
| texts/trim read | 13.5 ns | 4.8 ns | 2.82× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 333 | 113 | 65 | 23 |
| maps/lookup 1000000 ints | 39.90 ms | 19.82 ms | 2.01× ⚠ | 0 | 0 | 0 | 0 | 99.75M | 108.30M | 195.30M | 28.70M |
| gzip/compress 1MiB | 7.75 ms | 3.96 ms | 1.95× ⚠ | 8 +1 in regions | 27 | 883.3k +128.9k in regions | 1.28M | 69.89M | 67.80M | 37.97M | 19.50M |
| encoding/base64 decode 1MiB | 1.52 ms | 782.93 µs | 1.94× ⚠ | 2 ⚠ +1 in regions | 2 | 2.00M +1024.0k in regions | 1.00M | 43.46M | 16.27M | 7.31M | 3.88M |
| files/readLines 4MiB | 4.88 ms | 2.65 ms | 1.84× ⚠ | 151 +95326 in regions | 95329 | 130.8k +7.27M in regions | 4.37M | 70.62M | 37.50M | 20.00M | 10.88M |
| texts/chars ascii | 463.0 ns | 307.3 ns | 1.51× ⚠ | 0 | 0 | 0 | 0 | 10.0k | 9.9k | 2.2k | 1.5k |
| files/writeLines 4MiB | 3.51 ms | 2.43 ms | 1.45× ⚠ | 965 ⚠ +1 in regions | 4 | 11.00M +128 in regions | 4.1k | 31.65M | 11.75M | 8.11M | 3.59M |
| texts/split lines 1MiB | 232.24 µs | 169.68 µs | 1.37× ⚠ | 0 +1 in regions | 1 | 0 +1.89M in regions | 376.0k | 4.69M | 3.59M | 1.11M | 973.4k |
| tasks/spawn 1000 | 179.97 µs | 159.94 µs | 1.13× ⚠ | 1952 ⚠ | 2002 | 648.0k | 78.3k | 2.24M | 2.31M | 1.39M | 1.75M |
| texts/fromBytes other | 60.72 µs | 54.09 µs (47.49 µs–57.80 µs) | 1.12× ⚠ | 1 | 1 | 1024.0k | 1.00M | 972.9k | 547.8k | 296.9k | 315.0k |
| http/json 64x100 | 30.66 ms | 27.37 ms | 1.12× ⚠ | 454954 ⚠ +8709 in regions | 683953 | 137.67M +609.1k in regions | 53.13M | 634.24M | 3.48G | 1.74G | 5.79G |
| ffi/from a C thread 1000 | 65.22 µs | 60.72 µs | 1.07× ⚠ | 2 ⚠ | 0 | 624 | 0 | 1.8k | 516.2k | 4.7k | 162.5k |
| programs/sum lines | 15.26 µs | 14.57 µs | 1.05× ⚠ | 1 ⚠ +1 in regions | 2 | 4.7k +128 in regions | 4.0k | 299.7k | 299.2k | 73.8k | 71.1k |
| ffi/from C threads 8x1000 | 181.27 µs | 174.34 µs | 1.04× ⚠ | 16 ⚠ | 0 | 4.9k | 0 | 13.1k | 4.12M | 35.3k | 1.47M |
| json/decode 1MiB | 8.94 ms | 8.65 ms | 1.03× ⚠ | 12561 | 46552 | 2.02M | 4.97M | 138.94M | 173.85M | 43.20M | 50.11M |
| net/tls echo 1x1000 | 12.31 ms | 11.94 ms | 1.03× ⚠ | 4900 ⚠ +1890 in regions | 3744 | 105.8k +74.5k in regions | 50.5k | 18.54M | 14.29M | 14.14M | 11.72M |
| queries/uniqueBy 10000 | 141.53 µs | 137.48 µs | 1.03× ⚠ | 0 +40 in regions | 32 | 0 +128.2k in regions | 160.2k | 2.09M | 1.98M | 705.8k | 748.9k |
| floats/sqrt 100000 | 179.09 µs | 175.65 µs | 1.02× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.0k | 897.0k | 876.9k |
| floats/mandelbrot 64 | 139.26 µs | 138.81 µs | 1.00× ⚠ | 0 | 0 | 0 | 0 | 1.48M | 1.57M | 687.4k | 686.0k |
| queries/iter filter sumBy 10000 | 6.23 µs | 6.22 µs | 1.00× ⚠ | 0 +1 in regions | 0 | 0 +48 in regions | 0 | 160.2k | 77.4k | 30.1k | 28.3k |
| arrays/map 1000000 | 854.56 µs | 854.22 µs | 1.00× ⚠ | 0 +1 in regions | 1 | 0 +7.63M in regions | 7.63M | 19.51M | 11.59M | 4.08M | 3.90M |
| programs/count words | 10.21 µs | 10.21 µs | 1.00× ⚠ | 0 +7 in regions | 3 | 0 +25.1k in regions | 9.8k | 189.5k | 186.6k | 49.1k | 53.0k |
| hash/sha256 1MiB | 424.34 µs | 426.69 µs | 0.99× | 2 ⚠ +1 in regions | 0 | 136 +64 in regions | 0 | 2.69M | 2.54M | 2.12M | 2.12M |
| queries/joinBy 10000x1000 | 450.98 µs | 457.06 µs (441.70 µs–558.30 µs) | 0.99× | 1045 ⚠ | 1039 | 1.67M | 2.91M | 5.99M | 13.82M | 2.17M | 6.34M |
| net/echo 1x1000 | 10.69 ms | 10.89 ms | 0.98× | 900 +1890 in regions | 3744 | 28.1k +74.5k in regions | 50.4k | 10.49M | 6.99M | 8.74M | 7.12M |
| arrays/index sum 10000 | 2.07 µs | 2.14 µs | 0.97× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.5k |
| runtime/mutex 4x1000 | 20.26 µs | 21.03 µs | 0.96× | 5 ⚠ +1 in regions | 6 | 400 +32 in regions | 224 | 408.5k | 120.2k | 94.8k | 152.5k |
| queries/countBy 10000 | 102.27 µs | 110.54 µs | 0.93× | 0 +7 in regions | 2 | 0 +352 in regions | 256 | 1.75M | 1.58M | 491.4k | 568.4k |
| processes/true by name | 318.38 µs | 347.55 µs | 0.92× | 21 | 47 | 1.1k | 9.4k | 14.3k | 234.8k | 31.5k | 263.6k |
| processes/true by path | 311.00 µs | 340.43 µs | 0.91× | 19 | 34 | 1015 | 8.3k | 13.0k | 225.7k | 29.6k | 252.7k |
| strings/builder 100 | 724.2 ns | 806.7 ns | 0.90× | 0 +5 in regions | 8 | 0 +1.2k in regions | 1.9k | 15.5k | 13.4k | 3.5k | 4.3k |
| http/tls 64x100 | 66.86 ms | 74.56 ms | 0.90× | 378607 ⚠ +6405 in regions | 451076 | 52.63M +701.1k in regions | 49.17M | 587.51M | 1.78G | 918.84M | 1.56G |
| texts/parse ints 1000 | 5.59 µs | 6.41 µs | 0.87× | 0 | 0 | 0 | 0 | 133.1k | 144.4k | 27.1k | 30.8k |
| ffi/waits from a C thread 100 | 915.55 µs | 1.05 ms | 0.87× | 109 | 200 | 5.3k | 12.5k | 236.5k | 2.42M | 448.8k | 2.18M |
| texts/split other | 1.20 µs | 1.39 µs | 0.87× | 0 +1 in regions | 1 | 0 +9.4k in regions | 3.4k | 25.2k | 26.9k | 5.8k | 7.9k |
| http/echo 1MiB | 579.37 µs | 671.44 µs | 0.86× | 124 ⚠ | 148 | 10.36M | 4.31M | 1.07M | 4.55M | 1.30M | 4.18M |
| json/decode struct | 1.05 µs | 1.23 µs | 0.86× | 1 | 4 | 64 | 288 | 16.3k | 21.0k | 5.1k | 6.2k |
| runtime/join 1 | 244.8 ns | 287.1 ns | 0.85× | 1 | 2 | 40 | 144 | 2.7k | 3.2k | 1.2k | 1.5k |
| json/encode 1MiB | 2.70 ms | 3.20 ms | 0.84× | 16 ⚠ +1 in regions | 4 | 2.00M +1.00M in regions | 1.07M | 47.40M | — | 13.05M | — |
| runtime/select 1000 | 121.14 µs | 144.79 µs | 0.84× | 4 ⚠ | 4 | 723 | 528 | 1.31M | 2.09M | 539.2k | 714.6k |
| strings/contains 1000 | 13.1 ns | 15.7 ns | 0.83× | 0 | 0 | 0 | 0 | 320 | 394 | 63 | 75 |
| http/hello 1x1000 | 31.34 ms | 37.67 ms | 0.83× | 49000 ⚠ +1000 in regions | 69030 | 19.58M +109.4k in regions | 6.11M | 67.67M | 142.75M | 79.62M | 129.31M |
| loops/sum 1000000 | 609.94 µs | 735.32 µs | 0.83× | 0 | 0 | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.60M |
| queries/numbers iter map filter sum 10000 | 5.32 µs | 6.43 µs | 0.83× | 0 +2 in regions | 0 | 0 +96 in regions | 0 | 160.3k | 144.0k | 25.6k | 31.8k |
| loops/collatz 10000 | 750.53 µs | 910.06 µs | 0.82× | 0 | 0 | 0 | 0 | 14.54M | 8.86M | 3.66M | 4.47M |
| texts/chars other | 1.01 µs | 1.23 µs | 0.82× | 0 | 0 | 0 | 0 | 12.8k | 28.2k | 5.0k | 5.8k |
| queries/maxBy 10000 | 6.69 µs | 8.16 µs | 0.82× | 0 | 0 | 0 | 0 | 145.2k | 147.8k | 32.6k | 39.1k |
| json/encode 100 structs | 23.23 µs | 28.59 µs | 0.81× | 1 +1 in regions | 4 | 1 +7.2k in regions | 16.1k | 407.1k | 441.3k | 111.5k | 143.8k |
| http/hello 64x100 | 18.74 ms | 24.00 ms | 0.78× | 313890 ⚠ +6405 in regions | 435564 | 125.03M +701.1k in regions | 39.09M | 429.63M | 3.03G | 1.07G | 5.06G |
| files/readText | 105.97 µs | 136.80 µs (115.94 µs–187.75 µs) | 0.77× | 5 +1 in regions | 6 | 243 +1024.0k in regions | 2.00M | 880.7k | 1.38M | 236.3k | 1.18M |
| programs/reprice order | 1.21 µs | 1.59 µs | 0.76× | 1 +1 in regions | 9 | 64 +128 in regions | 624 | 16.8k | 20.9k | 5.9k | 7.9k |
| queries/groupBy 1000000 | 28.96 ms | 38.70 ms | 0.75× | 29613 +1 in regions | 36912 | 78.86M +48 in regions | 95.25M | 206.49M | 565.51M | 143.18M | 370.25M |
| strings/split 100 | 578.6 ns | 774.2 ns | 0.75× | 0 +1 in regions | 1 | 0 +4.4k in regions | 1.8k | 12.0k | 13.7k | 2.8k | 3.9k |
| maps/lookup 10000 ints | 54.42 µs | 74.43 µs | 0.73× | 0 | 0 | 0 | 0 | 970.1k | 1.06M | 259.5k | 366.0k |
| encoding/base64 encode 1MiB | 536.04 µs | 736.49 µs | 0.73× | 1 +1 in regions | 2 | 1.33M +1.33M in regions | 2.67M | 12.03M | 15.65M | 2.57M | 3.76M |
| calls/fib 25 | 208.57 µs | 288.90 µs | 0.72× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.01M | 1.37M |
| tasks/channel 10000 | 190.62 µs | 273.75 µs | 0.70× | 2 ⚠ | 2 | 744 | 664 | 3.15M | 4.41M | 906.1k | 1.31M |
| strings/int to text 1000 | 7.14 µs | 10.91 µs | 0.65× | 0 | 900 | 0 | 2.8k | 132.3k | 211.8k | 34.4k | 52.5k |
| texts/join 100 | 388.5 ns | 605.0 ns | 0.64× | 0 +1 in regions | 1 | 0 +1.0k in regions | 576 | 6.8k | 11.5k | 1.9k | 3.0k |
| programs/sort adults | 162.59 µs | 256.62 µs | 0.63× | 12 +697 in regions | 710 | 75.7k +58.1k in regions | 90.4k | 2.43M | 4.05M | 781.6k | 1.26M |
| queries/filter sumBy 10000 | 53.00 µs | 85.59 µs (82.13 µs–104.68 µs) | 0.62× | 0 +14 in regions | 16 | 0 +512.3k in regions | 727.7k | 528.2k | 2.45M | 256.3k | 1.18M |
| queries/numbers map filter sum 10000 | 16.16 µs | 28.36 µs (25.91 µs–29.19 µs) | 0.57× | 0 +14 in regions | 16 | 0 +142.6k in regions | 165.2k | 323.5k | 611.2k | 77.2k | 251.4k |
| queries/groupBy 10000 | 244.13 µs | 431.10 µs | 0.57× | 2683 +1 in regions | 4664 | 879.9k +48 in regions | 1.18M | 2.87M | 7.02M | 1.17M | 3.57M |
| texts/toUpper other 1MiB | 2.61 ms | 4.74 ms | 0.55× | 0 +1 in regions | 1 | 0 +1024.0k in regions | 1.00M | 41.27M | 103.10M | 12.68M | 22.93M |
| texts/toUpper other | 4.16 µs | 8.06 µs | 0.52× | 0 +1 in regions | 1 | 0 +1.7k in regions | 1.8k | 66.5k | 173.3k | 20.1k | 39.3k |
| strings/interpolate | 17.3 ns | 34.1 ns | 0.51× | 0 +1 in regions | 1 | 0 +64 in regions | 32 | 368 | 664 | 83 | 155 |
| calls/closure 1000 | 203.9 ns | 411.4 ns | 0.50× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 2.0k |
| texts/replace ascii | 563.2 ns | 1.18 µs | 0.48× | 0 +1 in regions | 1 | 0 +1.0k in regions | 1.0k | 10.8k | 24.6k | 2.6k | 5.9k |
| json/parse object | 1.66 µs | 3.52 µs | 0.47× | 14 ⚠ | 36 | 1.7k | 1.5k | 26.1k | 47.7k | 8.0k | 18.1k |
| texts/fromBytes ascii | 24.29 µs | 54.89 µs (44.12 µs–57.23 µs) | 0.44× | 1 ⚠ | 1 | 1.00M | 1.00M | 252.6k | 479.5k | 117.5k | 239.6k |
| queries/averageBy 10000 | 10.41 µs | 24.10 µs | 0.43× | 0 | 0 | 0 | 0 | 200.2k | 53.8k | 50.1k | 102.8k |
| queries/toSet 10000 | 195.62 µs | 458.47 µs | 0.43× | 0 +42 in regions | 81 | 0 +1.00M in regions | 577.8k | 2.41M | 5.89M | 944.0k | 2.56M |
| arrays/map filter sum 10000 | 15.99 µs | 39.43 µs | 0.41× | 0 +12 in regions | 16 | 0 +142.5k in regions | 165.2k | 323.5k | 477.9k | 76.7k | 254.8k |
| json/parse 1000 numbers | 51.44 µs | 126.98 µs | 0.41× | 9 | 2015 | 32.2k | 54.1k | 956.1k | 2.12M | 247.6k | 630.8k |
| texts/replace other 1MiB | 212.71 µs | 534.29 µs | 0.40× | 17 ⚠ +1 in regions | 1 | 2.91M +1.01M in regions | 1.02M | 3.86M | 10.58M | 1.02M | 2.69M |
| texts/replace other | 363.3 ns | 1.00 µs | 0.36× | 0 +1 in regions | 1 | 0 +1.7k in regions | 1.8k | 7.3k | 19.6k | 1.7k | 5.3k |
| queries/sortedBy 10000 | 1.37 ms | 3.81 ms | 0.36× | 3 ⚠ | 2 | 1.07M | 392.0k | 15.53M | 28.42M | 6.54M | 18.22M |
| structs/users 1000 | 11.48 µs | 32.60 µs | 0.35× | 0 +937 in regions | 1910 | 0 +59.5k in regions | 72.8k | 241.4k | 664.9k | 55.2k | 195.5k |
| programs/build tree | 41.43 µs | 118.03 µs | 0.35× | 0 +1365 in regions | 4095 | 0 +128.0k in regions | 298.6k | 863.0k | 2.20M | 199.3k | 716.5k |
| ffi/callback 1000 | 10.43 µs | 30.11 µs | 0.35× | 0 | 0 | 0 | 0 | 140.2k | 497.3k | 50.2k | 135.8k |
| structs/points 1000 | 1.31 µs | 3.90 µs | 0.34× | 0 +1 in regions | 12 | 0 +15.7k in regions | 49.2k | 30.3k | 79.6k | 6.3k | 44.5k |
| maps/insert 1000 strings | 29.44 µs | 91.41 µs | 0.32× | 0 +518 in regions | 1920 | 0 +102.9k in regions | 121.0k | 486.6k | 1.30M | 141.7k | 468.7k |
| gzip/decompress 1MiB | 499.90 µs | 1.56 ms | 0.32× | 19 +1 in regions | 68 | 2.08M +1.00M in regions | 2.17M | 6.35M | 22.17M | 2.41M | 8.29M |
| texts/contains other | 149.3 ns | 473.3 ns | 0.32× | 0 | 0 | 0 | 0 | 3.1k | 8.2k | 719 | 2.3k |
| queries/keyedBy 10000 | 179.19 µs | 596.96 µs | 0.30× | 0 +43 in regions | 81 | 0 +2.00M in regions | 1.75M | 2.76M | 9.33M | 853.9k | 4.72M |
| maps/insert 10000 ints | 137.88 µs | 469.66 µs | 0.29× | 0 +42 in regions | 79 | 0 +1.25M in regions | 577.6k | 2.32M | 5.33M | 662.0k | 2.24M |
| maps/insert 1000000 ints | 18.25 ms | 74.21 ms | 0.25× | 0 +60 in regions | 8195 | 0 +80.00M in regions | 71.97M | 222.15M | 605.30M | 87.44M | 352.19M |
| regex/match 1000 | 35.28 µs | 155.48 µs | 0.23× | 0 | 0 | 0 | 0 | 499.6k | 2.99M | 171.6k | 740.1k |
| arrays/sort 1000000 | 10.62 ms | 47.86 ms | 0.22× | 0 +1 in regions | 1 | 0 +7.63M in regions | 7.63M | 192.40M | 296.92M | 51.24M | 231.64M |
| floats/add 100000 | 17.84 µs | 84.09 µs | 0.21× | 1 | 1 | 781.3k | 784.0k | 280.9k | 1.75M | 86.0k | 648.1k |
| arrays/sort 10000 | 64.16 µs | 360.52 µs | 0.18× | 0 +1 in regions | 1 | 0 +78.2k in regions | 80.0k | 1.32M | 2.42M | 308.3k | 1.72M |
| tensors/matmul 64 | 83.32 µs | 492.11 µs | 0.17× | 0 +2 in regions | 1 | 0 +32.1k in regions | 32.0k | 1.59M | 4.30M | 404.6k | 2.43M |
| arrays/push 1000 | 518.7 ns | 3.35 µs (3.23 µs–6.15 µs) | 0.15× | 0 +1 in regions | 12 | 0 +7.8k in regions | 24.6k | 12.8k | 41.2k | 2.5k | 21.5k |
| tensors/broadcast 256 | 10.37 µs | 75.46 µs (74.77 µs–107.84 µs) | 0.14× | 1 ⚠ +1 in regions | 1 | 512.0k +48 in regions | 512.0k | 190.2k | 1.44M | 50.6k | 560.6k |
| texts/contains 1MiB | 31.89 µs | 270.04 µs | 0.12× | 0 | 0 | 0 | 0 | 833.3k | 4.68M | 153.6k | 1.30M |
| floats/dot 100000 | 16.20 µs | 240.93 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 999.2k | 78.5k | 1.20M |
| floats/sum 100000 | 7.88 µs | 240.55 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 600.4k | 38.8k | 1.20M |
| texts/toUpper ascii | 54.5 ns | 2.29 µs | 0.02× | 0 +1 in regions | 1 | 0 +1.1k in regions | 1.1k | 1.4k | 51.6k | 263 | 11.2k |
| regex/findAll 1MiB | 39.40 µs | 14.67 ms | 0.00× | 450 ⚠ +151 in regions | 160 | 37.5k +7.1k in regions | 9.9k | 600.7k | 301.72M | 190.8k | 73.14M |

24 of 101 benchmarks are slower than Go; 25 allocate more; 27 run more instructions than Go.

Go's time is the median of its 5 runs of each row; 8 rows whose runs spread past a tenth give their range beside it.

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 53 µs | 287 µs | 84 µs | 1415 µs |
| http/json 20000/s | 77 µs | 204 µs | 127 µs | 1386 µs |
| http/tls hello 20000/s | 91 µs ⚠ | 90 µs | 150 µs | 1559 µs |

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/uniqueBy 10000 | 141.53 µs | 61.22 µs | 2.31× ⚠ | 0 | 65.2k |
| queries/iter filter sumBy 10000 | 6.23 µs | 54.96 µs | 0.11× | 0 | 96 |
| queries/joinBy 10000x1000 | 450.98 µs | 134.67 µs | 3.35× ⚠ | 1.67M | 102.3k |
| queries/countBy 10000 | 102.27 µs | 82.61 µs | 1.24× ⚠ | 0 | 1.1k |
| queries/numbers iter map filter sum 10000 | 5.32 µs | 23.85 µs | 0.22× | 0 | 128 |
| queries/maxBy 10000 | 6.69 µs | 16.35 µs | 0.41× | 0 | 40 |
| queries/filter sumBy 10000 | 53.00 µs | 48.10 µs | 1.10× ⚠ | 0 | 39.0k |
| queries/numbers map filter sum 10000 | 16.16 µs | 10.89 µs | 1.48× ⚠ | 0 | 51.7k |
| queries/groupBy 10000 | 244.13 µs | 278.99 µs | 0.88× | 879.9k | 583.9k |
| queries/averageBy 10000 | 10.41 µs | 25.75 µs | 0.40× | 0 | 24 |
| queries/toSet 10000 | 195.62 µs | 48.30 µs | 4.05× ⚠ | 0 | 158.0k |
| queries/sortedBy 10000 | 1.37 ms | 519.37 µs | 2.64× ⚠ | 1.07M | 234.7k |
| queries/keyedBy 10000 | 179.19 µs | 72.92 µs | 2.46× ⚠ | 0 | 276.4k |

77 of the 101 rows are at or under Go's time. The table before this one
gave 70 of 80, with one run of Go a row, which is Go at its slowest where
it allocates: its collector runs at another moment from run to run. With
five runs and their median, Go's rows that allocate much are faster than
that one run said, and of those 80 rows 65 are at or under Go's time: three
that read ahead of Go are behind it by more than a tenth, `tasks/spawn`
1.13×, `http/json 64x100` 1.12× and `texts/fromBytes other` 1.12×, three
more within a few hundredths, `queries/uniqueBy` and two on Go's time, and
`loops/collatz` is back under. Of the 21 rows new since, 12 are at or
under. The earlier tables flattered Tondo where Go allocates.

The rows still behind Go, and why, those past a tenth first:

- `net/echo 1MiB`, 11.7×: a stream's read gives a new array of up to 64
  KiB, which the program extends, where Go reads into a buffer made once;
  33.7 million instructions against Go's 3.2 thousand.
- `texts/trim`, 2.87×, and `texts/trim read`, 2.82×: trim reads ASCII a
  byte at a time and gives a text with nothing to trim back as it is
  (a2d299d7), but a trimmed text is still a new one holding its bytes,
  where Go's is a view of the old: a region takes it, which costs the heap
  nothing, but the call and the copy remain, 326 and 333 instructions
  against Go's 115 and 113, of which the copy and its region are about
  half. `texts/trim read` measures the commoner case, a trimmed text only
  read, `padded.trim().byteLength()` against `len(strings.TrimSpace(padded))`.
  A view of the text would close both; lending a part only read was built,
  measured and set aside, as `docs/decisions.md` says. What is left to try
  is in docs/pending.md.
- `maps/lookup 1000000 ints`, 2.01×: a million reads in a random order
  miss the caches, with fewer instructions than Go, 99.8 million against
  108.3.
- `gzip/compress 1MiB`, 1.95×, and 3.8% more bytes than Go at the default
  level; the sizes are below.
- `encoding/base64 decode 1MiB`, 1.94×: the library checks the text and
  then decodes it, two passes, and copies the bytes into their region.
- `files/readLines 4MiB`, 1.84×: a text for each line, about 740
  instructions a line against Go's 390.
- `texts/chars ascii`, 1.51×: Go's loop is in its fast placement, 307.3
  ns, with the instructions it had at 457.5 ns in a table before. Tondo's
  10.0 thousand instructions are those of main before piece 1: laying the
  decoding's way out apart took it to 8.9 thousand, and since dead values
  go, regalloc2 copies the loop's count on its way back, as the count
  needs a register that survives the exit's call of the runtime. That copy
  is the first piece of step 3 of docs/pending.md.
- `files/writeLines 4MiB`, 1.45×: 965 allocations and 11 MB on the heap a
  run, where Go's bufio.Writer makes 4.
- `texts/split lines 1MiB`, 1.37×: each of 23,832 pieces is a text of its
  own, where Go's are views of the first.
- `tasks/spawn 1000`, 1.13×: each task's record with its join, about 650
  bytes, where Go takes its goroutines' from a list of those that ended.
- `http/json 64x100`, 1.12×: two thirds of Go's allocations and 2.6 times
  its bytes, about 21 KB a request against Go's 8.3.
- `texts/fromBytes other`, 1.12×: Go's five runs took 47.5 to 57.8 µs,
  and the check that the bytes are UTF-8 is what Tondo adds.

Within a tenth: `ffi/from a C thread` 1.07× and `ffi/from C threads`
1.04×, a thread of C new to the runtime costing it the records it keeps
for a thread; `programs/sum lines` 1.05×, a text for each line; and
`json/decode 1MiB` 1.03×, `net/tls echo` 1.03×, `queries/uniqueBy` 1.03×
and `floats/sqrt` 1.02×. `floats/mandelbrot`, `queries/iter filter
sumBy`, `arrays/map 1000000` and `programs/count words` are on Go's time,
within a third of a percent.

The rows of large data, each the work of a smaller row at a size where
caches, the allocator's large blocks, regions that grow and Go's
collector behave otherwise. Those far ahead: `regex/findAll 1MiB`, 39.4
µs against Go's 14.67 ms, where the regex looks for the pattern's literal
with vectors before it tries the rest; `texts/contains 1MiB` 0.12×; the
sort and the insert of a million, 0.22× and 0.25×; `texts/replace other
1MiB` 0.40×; `gzip/decompress 1MiB` 0.32×; and `texts/toUpper other
1MiB` 0.55×. Those far behind are in the list above: the stream, the map
read at random, gzip's compression, base64's decode, the lines read,
written and split.

gzip with zlib-rs, on the mebibyte of JSON of `gzip/compress 1MiB`,
writes more than miniz_oxide did, the backend before, and more than Go:

| Level | zlib-rs | miniz_oxide | Go |
|---|---:|---:|---:|
| 1 | 291,507 | 244,071 | 194,943 |
| 6, the default | 131,987 | 121,275 | 127,166 |
| 9 | 120,907 | 119,569 | 117,168 |

The default level was weighed and kept at 6: a program gets its answer in
half the time miniz_oxide took for 8.8% more bytes, and one that wants
the old size asks for 9, which still takes a fifth less time. The
maintainer: "Lo hacemos así".

The two rows that start a program, `processes/true by name` and `by path`,
at 0.92× and 0.91×, allocate 21 and 19 times a run against Go's 47 and
34: a child inherits the system's environment while it is the program's
copy.

Three rows read about a mebibyte of text whole, as a file or a response is
read: `texts/fromBytes ascii` and `texts/fromBytes other` make text of
bytes, and `files/readText` reads a file that the page cache holds, into
the text itself since it is large. Go copies the bytes into a string as
they are; Tondo checks that they are UTF-8 too, which is most of the
difference they show, and the most where the text is not ASCII. They read
0.44×, 1.12× and 0.77×; Go's five runs of each spread past a tenth.

The table before this one, at a9dd4a58, gave 70 of 80 rows at or under
Go's time with one run of Go a row. Against the table taken at 487a19d9
with five, the one with the rows of large data before the changes that
followed them, Go's times have a median of 0.998 of those and three of its
rows moved by more than a tenth.

`B/op` counts what each side's allocator gives. Where Tondo allocates as
many times as Go but more bytes, as `strings/split` in its region, its
pieces are texts of their own, each with a header and its bytes, where Go's
are views of the first.

The servers of the `http` rows at 20000 requests a second, each sent when
due whether or not those before came back, its latency from then until its
answer, after half a second that opens the connections (`bench/latency`):

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 53 µs | 287 µs | 84 µs | 1415 µs |
| http/json 20000/s | 77 µs | 204 µs | 127 µs | 1386 µs |
| http/tls hello 20000/s | 91 µs ⚠ | 90 µs | 150 µs | 1559 µs |

Tondo's median is under Go's on the servers in the clear and a
microsecond over it on `http/tls hello`, 91 µs against 90; its 99th
percentile is about a tenth of Go's on every server.

The rows that LINQ says something of, against C# (`bench/csharp`, measured
as `go test -bench` measures, its bytes from the allocator and no count of
allocations):

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/uniqueBy 10000 | 141.53 µs | 61.22 µs | 2.31× ⚠ | 0 | 65.2k |
| queries/iter filter sumBy 10000 | 6.23 µs | 54.96 µs | 0.11× | 0 | 96 |
| queries/joinBy 10000x1000 | 450.98 µs | 134.67 µs | 3.35× ⚠ | 1.67M | 102.3k |
| queries/countBy 10000 | 102.27 µs | 82.61 µs | 1.24× ⚠ | 0 | 1.1k |
| queries/numbers iter map filter sum 10000 | 5.32 µs | 23.85 µs | 0.22× | 0 | 128 |
| queries/maxBy 10000 | 6.69 µs | 16.35 µs | 0.41× | 0 | 40 |
| queries/filter sumBy 10000 | 53.00 µs | 48.10 µs | 1.10× ⚠ | 0 | 39.0k |
| queries/numbers map filter sum 10000 | 16.16 µs | 10.89 µs | 1.48× ⚠ | 0 | 51.7k |
| queries/groupBy 10000 | 244.13 µs | 278.99 µs | 0.88× | 879.9k | 583.9k |
| queries/averageBy 10000 | 10.41 µs | 25.75 µs | 0.40× | 0 | 24 |
| queries/toSet 10000 | 195.62 µs | 48.30 µs | 4.05× ⚠ | 0 | 158.0k |
| queries/sortedBy 10000 | 1.37 ms | 519.37 µs | 2.64× ⚠ | 1.07M | 234.7k |
| queries/keyedBy 10000 | 179.19 µs | 72.92 µs | 2.46× ⚠ | 0 | 276.4k |

Tondo is ahead of C# on both lazy chains, nine times on the one over
records and four times on the other, on `maxBy`, `averageBy` and
`groupBy`; C# on the rest, 1.1 to 1.5 times on the eager chains, 1.2
times on `countBy`, and two to four times on sorting, on sets and on most
maps. C#'s times move more
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
