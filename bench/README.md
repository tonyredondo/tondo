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
first on `PATH`), on 2026-11-26 at a9dd4a58, in an environment of 55
variables, as the rows that start a program move with their number: Go
copies them at each start, and Tondo gives a child its program's copy
once the program has changed its environment. `HOME`, `PATH`, `TONDO` and
`TABLE_01` to `TABLE_52`, each `abcdefghijklmnopqrstuvwxyz0123`, are given
to `env -i` with no shell between it and `tondo run bench/compare.to`;
each side for its default level of processor:
Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`, as the tables of
latency and of C# below it. Go's side is one run of each row, `go test
-bench -benchtime 1s`, whose time moves more from one run to the next than
Tondo's, by a tenth or more on rows that allocate much; compare.to will
take five and their median (docs/pending.md). A row is marked when it is
slower than Go, and
its allocations when it allocates more, in number or in bytes, on the
heap: the columns of allocations and bytes are what each side's allocator
gives, which compare with Go's, and beside Tondo's, what it made in
regions besides, which Go would give its heap where the value leaves its
function:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| texts/trim read | 14.0 ns | 4.9 ns | 2.87× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 333 | 113 | 66 | 23 |
| texts/trim | 14.0 ns | 4.9 ns | 2.86× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 326 | 115 | 66 | 23 |
| texts/chars ascii | 469.1 ns | 313.1 ns | 1.50× ⚠ | 0 | 0 | 0 | 0 | 10.0k | 9.9k | 2.3k | 1.5k |
| net/tls echo 1x1000 | 12.91 ms | 12.30 ms | 1.05× ⚠ | 4900 ⚠ +1890 in regions | 3744 | 105.8k +74.5k in regions | 50.6k | 18.54M | 14.31M | 14.68M | 11.83M |
| loops/collatz 10000 | 902.40 µs | 866.60 µs | 1.04× ⚠ | 0 | 0 | 0 | 0 | 14.54M | 8.85M | 4.33M | 4.14M |
| ffi/from a C thread 1000 | 66.45 µs | 63.83 µs | 1.04× ⚠ | 2 ⚠ | 0 | 625 | 0 | 1.8k | 516.1k | 4.8k | 163.1k |
| programs/sum lines | 15.65 µs | 15.07 µs | 1.04× ⚠ | 1 ⚠ +1 in regions | 2 | 4.7k +128 in regions | 4.0k | 299.7k | 298.2k | 74.0k | 71.1k |
| floats/sqrt 100000 | 187.34 µs | 180.95 µs | 1.04× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.0k | 900.2k | 864.0k |
| ffi/from C threads 8x1000 | 180.82 µs | 178.97 µs | 1.01× ⚠ | 16 ⚠ | 0 | 4.9k | 4 | 13.1k | 4.12M | 34.7k | 1.50M |
| queries/iter filter sumBy 10000 | 6.33 µs | 6.30 µs | 1.01× ⚠ | 0 +1 in regions | 0 | 0 +48 in regions | 0 | 160.2k | 80.0k | 30.1k | 29.8k |
| net/echo 1x1000 | 11.14 ms | 11.27 ms | 0.99× | 900 +1890 in regions | 3744 | 28.1k +74.5k in regions | 50.5k | 10.50M | 7.17M | 8.92M | 7.41M |
| floats/mandelbrot 64 | 139.65 µs | 142.81 µs | 0.98× | 0 | 0 | 0 | 0 | 1.48M | 1.57M | 687.5k | 685.6k |
| tasks/spawn 1000 | 182.89 µs | 189.42 µs | 0.97× | 1952 ⚠ | 2002 | 648.2k | 78.5k | 2.23M | 2.34M | 1.39M | 1.81M |
| calls/closure 1000 | 203.6 ns | 212.9 ns | 0.96× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| processes/true by name | 361.26 µs | 379.27 µs | 0.95× | 21 | 47 | 1.1k | 9.4k | 14.3k | 235.2k | 41.3k | 264.2k |
| runtime/mutex 4x1000 | 20.43 µs | 21.60 µs | 0.95× | 5 ⚠ +1 in regions | 6 | 400 +32 in regions | 224 | 408.5k | 119.7k | 95.4k | 141.6k |
| ffi/waits from a C thread 100 | 902.22 µs | 955.29 µs | 0.94× | 109 | 200 | 5.3k | 12.5k | 236.7k | 2.01M | 462.9k | 1.84M |
| processes/true by path | 358.13 µs | 382.12 µs | 0.94× | 19 | 34 | 1.0k | 8.3k | 13.0k | 227.7k | 32.2k | 267.9k |
| queries/countBy 10000 | 103.70 µs | 110.70 µs | 0.94× | 0 +7 in regions | 2 | 0 +352 in regions | 256 | 1.75M | 1.55M | 490.6k | 525.7k |
| queries/uniqueBy 10000 | 143.57 µs | 154.66 µs | 0.93× | 0 +40 in regions | 32 | 0 +128.2k in regions | 160.2k | 2.09M | 1.94M | 671.7k | 769.4k |
| programs/count words | 10.20 µs | 11.04 µs | 0.92× | 0 +7 in regions | 3 | 0 +25.1k in regions | 9.8k | 189.5k | 186.3k | 49.3k | 52.0k |
| texts/parse ints 1000 | 5.93 µs | 6.50 µs | 0.91× | 0 | 0 | 0 | 0 | 133.1k | 144.5k | 28.1k | 30.8k |
| arrays/index sum 10000 | 2.07 µs | 2.27 µs | 0.91× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.7k |
| strings/builder 100 | 738.6 ns | 827.8 ns | 0.89× | 0 +5 in regions | 8 | 0 +1.2k in regions | 1.9k | 15.5k | 13.4k | 3.4k | 4.3k |
| loops/sum 1000000 | 623.89 µs | 744.65 µs | 0.84× | 0 | 0 | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.54M |
| texts/chars other | 1.04 µs | 1.24 µs | 0.84× | 0 | 0 | 0 | 0 | 12.8k | 28.2k | 5.0k | 5.8k |
| http/json 64x100 | 34.45 ms | 41.41 ms | 0.83× | 455554 ⚠ +8709 in regions | 686176 | 136.41M +609.1k in regions | 55.24M | 630.04M | 3.73G | 1.60G | 6.05G |
| queries/maxBy 10000 | 6.83 µs | 8.44 µs | 0.81× | 0 | 0 | 0 | 0 | 145.2k | 150.1k | 32.6k | 39.4k |
| runtime/select 1000 | 121.89 µs | 150.88 µs | 0.81× | 4 ⚠ | 4 | 722 | 528 | 1.31M | 2.09M | 542.4k | 792.0k |
| json/decode struct | 1.02 µs | 1.27 µs | 0.80× | 1 | 4 | 64 | 288 | 16.3k | 21.1k | 4.8k | 6.0k |
| http/hello 1x1000 | 32.71 ms | 41.13 ms | 0.80× | 49000 ⚠ +1000 in regions | 69101 | 19.48M +109.4k in regions | 6.15M | 67.01M | 142.94M | 74.14M | 131.38M |
| runtime/join 1 | 260.1 ns | 327.7 ns | 0.79× | 1 | 2 | 40 | 144 | 2.7k | 3.3k | 1.2k | 1.5k |
| json/encode 100 structs | 24.17 µs | 30.88 µs | 0.78× | 1 +1 in regions | 4 | 1 +7.2k in regions | 16.1k | 407.1k | 440.3k | 114.2k | 147.0k |
| http/tls 64x100 | 69.00 ms | 88.15 ms | 0.78× | 378604 ⚠ +6405 in regions | 458820 | 51.84M +701.1k in regions | 50.92M | 584.11M | 1.71G | 953.98M | 1.47G |
| queries/numbers iter map filter sum 10000 | 5.66 µs | 7.27 µs | 0.78× | 0 +2 in regions | 0 | 0 +96 in regions | 0 | 160.3k | 142.8k | 26.6k | 34.1k |
| texts/fromBytes other | 62.25 µs | 80.42 µs | 0.77× | 1 | 1 | 1024.0k | 1.00M | 972.9k | 542.0k | 296.4k | 374.7k |
| strings/contains 1000 | 12.4 ns | 16.0 ns | 0.77× | 0 | 0 | 0 | 0 | 320 | 394 | 61 | 76 |
| strings/split 100 | 587.0 ns | 795.9 ns | 0.74× | 0 +1 in regions | 1 | 0 +4.4k in regions | 1.8k | 12.0k | 13.7k | 2.8k | 4.0k |
| maps/lookup 10000 ints | 54.91 µs | 75.96 µs | 0.72× | 0 | 0 | 0 | 0 | 970.1k | 1.06M | 257.9k | 355.1k |
| programs/reprice order | 1.21 µs | 1.67 µs | 0.72× | 1 +1 in regions | 9 | 64 +128 in regions | 627 | 16.8k | 20.9k | 5.7k | 8.0k |
| texts/split other | 1.26 µs | 1.78 µs | 0.71× | 0 +1 in regions | 1 | 0 +9.4k in regions | 3.4k | 25.2k | 27.1k | 5.9k | 7.5k |
| calls/fib 25 | 207.54 µs | 295.14 µs | 0.70× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.01M | 1.38M |
| tasks/channel 10000 | 188.32 µs | 283.61 µs | 0.66× | 2 ⚠ | 2 | 744 | 664 | 3.15M | 4.42M | 921.2k | 1.34M |
| strings/int to text 1000 | 7.30 µs | 11.06 µs | 0.66× | 0 | 900 | 0 | 2.8k | 132.3k | 211.3k | 34.4k | 52.9k |
| files/readText | 119.09 µs | 180.74 µs | 0.66× | 4 +1 in regions | 6 | 1.00M +1024.0k in regions | 2.00M | 983.0k | 1.42M | 318.5k | 1.77M |
| programs/sort adults | 161.72 µs | 272.08 µs | 0.59× | 12 +697 in regions | 710 | 75.7k +58.1k in regions | 90.4k | 2.43M | 4.02M | 774.2k | 1.26M |
| texts/join 100 | 392.9 ns | 669.5 ns | 0.59× | 0 +1 in regions | 1 | 0 +1.0k in regions | 576 | 6.8k | 11.5k | 1.9k | 3.0k |
| queries/joinBy 10000x1000 | 488.07 µs | 904.30 µs | 0.54× | 1045 ⚠ | 1039 | 1.67M | 2.91M | 5.98M | 13.05M | 2.30M | 5.73M |
| http/hello 64x100 | 19.50 ms | 37.17 ms | 0.52× | 313889 ⚠ +6405 in regions | 436756 | 124.29M +701.1k in regions | 39.83M | 425.54M | 2.97G | 1.07G | 4.95G |
| strings/interpolate | 17.6 ns | 34.3 ns | 0.51× | 0 +1 in regions | 1 | 0 +64 in regions | 32 | 368 | 663 | 83 | 161 |
| texts/toUpper other | 4.27 µs | 8.34 µs | 0.51× | 0 +1 in regions | 1 | 0 +1.7k in regions | 1.8k | 66.5k | 173.0k | 20.2k | 39.4k |
| json/parse object | 1.64 µs | 3.74 µs | 0.44× | 14 ⚠ | 36 | 1.7k | 1.5k | 26.1k | 48.0k | 7.7k | 17.8k |
| texts/replace ascii | 582.5 ns | 1.35 µs | 0.43× | 0 +1 in regions | 1 | 0 +1.0k in regions | 1.0k | 10.8k | 24.6k | 2.7k | 5.9k |
| arrays/map filter sum 10000 | 15.90 µs | 36.75 µs | 0.43× | 0 +12 in regions | 16 | 0 +142.5k in regions | 165.2k | 323.5k | 484.0k | 76.2k | 206.8k |
| queries/averageBy 10000 | 10.57 µs | 25.22 µs | 0.42× | 0 | 0 | 0 | 0 | 200.2k | 90.0k | 50.4k | 120.7k |
| texts/fromBytes ascii | 24.83 µs | 61.81 µs | 0.40× | 1 ⚠ | 1 | 1.00M | 1.00M | 252.6k | 340.8k | 118.0k | 292.4k |
| queries/toSet 10000 | 201.14 µs | 508.10 µs | 0.40× | 0 +42 in regions | 81 | 0 +1.00M in regions | 577.8k | 2.41M | 5.75M | 948.7k | 2.60M |
| queries/groupBy 10000 | 242.49 µs | 623.58 µs | 0.39× | 2683 +1 in regions | 4664 | 879.9k +48 in regions | 1.18M | 2.87M | 7.23M | 1.15M | 3.68M |
| json/parse 1000 numbers | 49.75 µs | 132.58 µs | 0.38× | 9 | 2015 | 32.2k | 54.2k | 956.1k | 2.12M | 233.4k | 629.8k |
| queries/numbers map filter sum 10000 | 16.20 µs | 44.42 µs | 0.36× | 0 +14 in regions | 16 | 0 +142.6k in regions | 165.2k | 323.5k | 649.2k | 76.3k | 276.9k |
| queries/sortedBy 10000 | 1.38 ms | 4.02 ms | 0.34× | 3 ⚠ | 2 | 1.07M | 392.0k | 15.53M | 30.51M | 6.57M | 19.19M |
| ffi/callback 1000 | 9.87 µs | 30.33 µs | 0.33× | 0 | 0 | 0 | 0 | 140.2k | 497.4k | 47.2k | 142.8k |
| texts/contains other | 156.1 ns | 483.2 ns | 0.32× | 0 | 0 | 0 | 0 | 3.1k | 8.2k | 741 | 2.3k |
| texts/replace other | 367.0 ns | 1.17 µs | 0.31× | 0 +1 in regions | 1 | 0 +1.7k in regions | 1.8k | 7.3k | 19.5k | 1.7k | 5.3k |
| queries/filter sumBy 10000 | 53.97 µs | 174.59 µs | 0.31× | 0 +14 in regions | 16 | 0 +512.3k in regions | 727.7k | 528.2k | 2.44M | 257.2k | 1.28M |
| programs/build tree | 42.29 µs | 137.69 µs | 0.31× | 0 +1365 in regions | 4095 | 0 +128.0k in regions | 298.6k | 863.0k | 2.19M | 199.8k | 694.4k |
| structs/users 1000 | 11.80 µs | 40.02 µs | 0.29× | 0 +937 in regions | 1910 | 0 +59.5k in regions | 72.8k | 241.4k | 664.4k | 55.5k | 196.2k |
| maps/insert 1000 strings | 29.34 µs | 101.97 µs | 0.29× | 0 +518 in regions | 1920 | 0 +102.9k in regions | 121.0k | 486.6k | 1.30M | 139.3k | 486.2k |
| maps/insert 10000 ints | 140.30 µs | 505.23 µs | 0.28× | 0 +42 in regions | 79 | 0 +1.25M in regions | 577.6k | 2.32M | 5.35M | 664.6k | 2.41M |
| queries/keyedBy 10000 | 182.09 µs | 730.50 µs | 0.25× | 0 +43 in regions | 81 | 0 +2.00M in regions | 1.75M | 2.76M | 9.31M | 857.4k | 4.46M |
| regex/match 1000 | 36.35 µs | 158.53 µs | 0.23× | 0 | 0 | 0 | 0 | 499.6k | 2.99M | 172.1k | 745.3k |
| floats/add 100000 | 18.32 µs | 100.27 µs | 0.18× | 1 | 1 | 781.3k | 784.0k | 281.0k | 1.77M | 86.8k | 673.2k |
| arrays/sort 10000 | 64.22 µs | 363.29 µs | 0.18× | 0 +1 in regions | 1 | 0 +78.2k in regions | 80.0k | 1.32M | 2.41M | 308.9k | 1.71M |
| tensors/matmul 64 | 84.85 µs | 504.95 µs | 0.17× | 0 +2 in regions | 1 | 0 +32.1k in regions | 32.0k | 1.59M | 4.30M | 403.3k | 2.42M |
| structs/points 1000 | 1.35 µs | 8.10 µs | 0.17× | 0 +1 in regions | 12 | 0 +15.7k in regions | 49.2k | 30.3k | 81.5k | 6.3k | 46.0k |
| arrays/push 1000 | 518.4 ns | 3.75 µs | 0.14× | 0 +1 in regions | 12 | 0 +7.8k in regions | 24.6k | 12.8k | 40.9k | 2.5k | 23.9k |
| tensors/broadcast 256 | 10.46 µs | 75.77 µs | 0.14× | 1 ⚠ +1 in regions | 1 | 512.0k +48 in regions | 512.0k | 190.2k | 1.44M | 50.0k | 560.2k |
| floats/dot 100000 | 16.66 µs | 251.19 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 1.00M | 78.7k | 1.21M |
| floats/sum 100000 | 8.33 µs | 248.84 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 600.3k | 38.3k | 1.21M |
| texts/toUpper ascii | 55.4 ns | 2.45 µs | 0.02× | 0 +1 in regions | 1 | 0 +1.1k in regions | 1.1k | 1.4k | 51.6k | 265 | 11.5k |

10 of 80 benchmarks are slower than Go; 17 allocate more; 19 run more instructions than Go.

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 50 µs | 389 µs | 78 µs | 1631 µs |
| http/json 20000/s | 75 µs | 269 µs | 131 µs | 1852 µs |
| http/tls hello 20000/s | 88 µs | 90 µs | 151 µs | 1584 µs |

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 6.33 µs | 65.62 µs | 0.10× | 0 | 96 |
| queries/countBy 10000 | 103.70 µs | 82.73 µs | 1.25× ⚠ | 0 | 1.1k |
| queries/uniqueBy 10000 | 143.57 µs | 61.64 µs | 2.33× ⚠ | 0 | 65.2k |
| queries/maxBy 10000 | 6.83 µs | 16.31 µs | 0.42× | 0 | 40 |
| queries/numbers iter map filter sum 10000 | 5.66 µs | 22.91 µs | 0.25× | 0 | 128 |
| queries/joinBy 10000x1000 | 488.07 µs | 134.40 µs | 3.63× ⚠ | 1.67M | 102.3k |
| queries/averageBy 10000 | 10.57 µs | 25.73 µs | 0.41× | 0 | 24 |
| queries/toSet 10000 | 201.14 µs | 50.16 µs | 4.01× ⚠ | 0 | 158.0k |
| queries/groupBy 10000 | 242.49 µs | 275.57 µs | 0.88× | 879.9k | 583.9k |
| queries/numbers map filter sum 10000 | 16.20 µs | 10.52 µs | 1.54× ⚠ | 0 | 51.7k |
| queries/sortedBy 10000 | 1.38 ms | 511.26 µs | 2.70× ⚠ | 1.07M | 234.7k |
| queries/filter sumBy 10000 | 53.97 µs | 47.56 µs | 1.13× ⚠ | 0 | 39.0k |
| queries/keyedBy 10000 | 182.09 µs | 73.07 µs | 2.49× ⚠ | 0 | 276.4k |

The rows still behind Go, and why:

- `texts/trim`, 2.86×, and `texts/trim read`, 2.87×: trim reads ASCII a
  byte at a time and gives a text with nothing to trim back as it is
  (a2d299d7), but a trimmed text is still a new one holding its bytes,
  where Go's is a view of the old: a region takes it, which costs the heap
  nothing, but the call and the copy remain, 326 and 333 instructions
  against Go's 115 and 113, of which the copy and its region are about
  half. `texts/trim read` measures the commoner case, a trimmed text only
  read, `padded.trim().byteLength()` against `len(strings.TrimSpace(padded))`.
  A view of the text would close both; lending a part only read was built,
  measured and set aside, as `docs/decisions.md` says. What is left to try
  is in docs/pending.md: `trim` in Tondo measured again, the inliner
  weighing a call by what inlining buys, and a region for the copy of a
  function over its budget.
- `texts/chars ascii`, 1.50×: Go's loop is in its fast placement in this
  run, 313.1 ns, where it took 457.5 ns in the table before with the same
  instructions, and the row reads as that table said it would against
  this time. Tondo's 10.0 thousand instructions are those of main before
  piece 1: laying the decoding's way out apart took it to 8.9 thousand,
  and since dead values go, regalloc2 copies the loop's count on its way
  back, as the count needs a register that survives the exit's call of the
  runtime. That copy is the first piece of step 3 of docs/pending.md.
- `net/tls echo`, 1.05×, which makes about one more allocation a round
  trip than Go, and `http/tls` a few more a request: rustls 0.23 makes a
  buffer for each record it encrypts and copies each one it reads into
  another, in its buffered API and its unbuffered one alike.
- `programs/sum lines`, 1.04×, from 1.61×: the second piece reads a text's
  reader where the text is and finds a line's end eight bytes at a time,
  299.7 thousand instructions against Go's 298.2, and one allocation
  against Go's two but 4.7 KB against 4.0. Each line is still a text of its
  own, in the regions of
  the loop's turns, where Go's scanner lends its buffer.
- `ffi/from a C thread`, 1.04×, and `ffi/from C threads`, 1.01×: a thread
  of C new to the runtime costs it the records it keeps for a thread, and
  each call a task's record in the thread's block.
- `floats/sqrt`, 1.04×: 725.0 thousand instructions against Go's 700.0, as
  in the table before, and 4% more cycles.
- `loops/collatz`, 1.04×, and `queries/iter filter sumBy`, 1.01×, were at
  0.82× and 0.85× in the table before with the same instructions: the
  second piece's measure against main finds both the same in four orders
  of the functions, collatz from 750 to 884 µs among them, and in this
  build their code lies where it runs slower.

Four rows that were behind in the table before are at or under Go's time
now: `texts/toUpper other`, 1.05× to 0.51×, by the second piece of pull
request 51, which changes a text's case a block at a time with 55% fewer
instructions; and `tasks/spawn`, 1.06× to 0.97×, `runtime/mutex`, 1.04× to
0.95×, and `floats/mandelbrot`, 1.00× to 0.98×, with the instructions they
had. docs/decisions.md says what each commit of the piece moved.

The two rows that start a program, `processes/true by name` and `by path`,
at 0.95× and 0.94×, allocate 21 and 19 times a run against Go's 47 and 34.
Since the second piece a child inherits the system's environment while it
is the program's copy, where a start copied it all, about seven
allocations and three thousand instructions a variable: 477 and 468
allocations in this table's 55 variables. In 200 variables, as a
session's shell has, they are at 0.90× and 0.91× of Go's time with the
same 21 and 19; docs/decisions.md gives both.

Three rows read about a mebibyte of text whole, as a file or a response is
read: `texts/fromBytes ascii` and `texts/fromBytes other` make text of
bytes, and `files/readText` reads a file that the page cache holds. Go
copies the bytes into a string as they are; Tondo checks that they are
UTF-8 too, which is most of the difference they show, and the most where
the text is not ASCII. Tondo's runs of them stay within about 2%; Go's
move by up to two thirds from one run to the next, as its collector, with
a mebibyte allocated at each run, runs at another moment: six samples
gave 35.0 to 58.8 µs, 52.3 to 73.2 and 140.3 to 236.5. In this table,
one run of Go a row, they read 0.40×, 0.77× and 0.66×, Go at 61.8, 80.4
and 180.7 µs.

The table before this one, at b5530f8b, gave 65 of 77 rows at or under
Go's time; this one gives 70 of 80, the three rows of a mebibyte new
since. Go's side is as it was then by its instructions, within 1% on 67
of 77 rows, at 4.76 GHz against 4.82 by the median of its rows' cycles
over their time; its times have a median of 1.025 of those, and a dozen
of its rows moved more than a tenth with the same instructions:
`texts/chars ascii` 32% faster, `http/hello 64x100` and `http/json 64x100`
twice as fast, and six rows of queries 11 to 22% slower.

`B/op` counts what each side's allocator gives. In `tasks/spawn` Tondo
allocates each task's record with its join, about 650 bytes, where Go takes
its goroutines' from a list of those that ended. Where Tondo allocates as
many times as Go but more bytes, as `strings/split` in its region, its
pieces are texts of their own, each with a header and its bytes, where Go's
are views of the first. `http/json` makes two thirds of Go's allocations
and 2.5 times its bytes, about 21 KB a request against Go's 8.6 in this
run, where Go's gave 15 in the table before.

The servers of the `http` rows at 20000 requests a second, each sent when
due whether or not those before came back, its latency from then until its
answer, after half a second that opens the connections (`bench/latency`):

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 50 µs | 389 µs | 78 µs | 1631 µs |
| http/json 20000/s | 75 µs | 269 µs | 131 µs | 1852 µs |
| http/tls hello 20000/s | 88 µs | 90 µs | 151 µs | 1584 µs |

Tondo's median is under Go's on every server, on `http/tls hello` by 2
µs, 88 against 90, where it was behind in the table before; its 99th
percentile is about a tenth of Go's on every server.

The rows that LINQ says something of, against C# (`bench/csharp`, measured
as `go test -bench` measures, its bytes from the allocator and no count of
allocations):

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/iter filter sumBy 10000 | 6.33 µs | 65.62 µs | 0.10× | 0 | 96 |
| queries/countBy 10000 | 103.70 µs | 82.73 µs | 1.25× ⚠ | 0 | 1.1k |
| queries/uniqueBy 10000 | 143.57 µs | 61.64 µs | 2.33× ⚠ | 0 | 65.2k |
| queries/maxBy 10000 | 6.83 µs | 16.31 µs | 0.42× | 0 | 40 |
| queries/numbers iter map filter sum 10000 | 5.66 µs | 22.91 µs | 0.25× | 0 | 128 |
| queries/joinBy 10000x1000 | 488.07 µs | 134.40 µs | 3.63× ⚠ | 1.67M | 102.3k |
| queries/averageBy 10000 | 10.57 µs | 25.73 µs | 0.41× | 0 | 24 |
| queries/toSet 10000 | 201.14 µs | 50.16 µs | 4.01× ⚠ | 0 | 158.0k |
| queries/groupBy 10000 | 242.49 µs | 275.57 µs | 0.88× | 879.9k | 583.9k |
| queries/numbers map filter sum 10000 | 16.20 µs | 10.52 µs | 1.54× ⚠ | 0 | 51.7k |
| queries/sortedBy 10000 | 1.38 ms | 511.26 µs | 2.70× ⚠ | 1.07M | 234.7k |
| queries/filter sumBy 10000 | 53.97 µs | 47.56 µs | 1.13× ⚠ | 0 | 39.0k |
| queries/keyedBy 10000 | 182.09 µs | 73.07 µs | 2.49× ⚠ | 0 | 276.4k |

Tondo is ahead of C# on both lazy chains, ten times on the one over
records and four times on the other, on `maxBy`, `averageBy` and
`groupBy`; C# on the rest, 1.1 to 1.5 times on the eager chains, 1.25
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
