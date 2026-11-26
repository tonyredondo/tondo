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
first on `PATH`), on 2026-11-26 at b5530f8b, in an environment of 55
variables, as a start of a program copies them all and the rows that start
one move with their number: `HOME`, `PATH`, `TONDO` and `TABLE_01` to
`TABLE_52`, each `abcdefghijklmnopqrstuvwxyz0123`, given to `env -i` with
no shell between it and `tondo run bench/compare.to`; each side for its
default level of processor:
Tondo's `baseline`, x86-64-v2, and Go's `GOAMD64=v1`, as the tables of
latency and of C# below it. A row is marked when it is slower than Go, and
its allocations when it allocates more, in number or in bytes, on the
heap: the columns of allocations and bytes are what each side's allocator
gives, which compare with Go's, and beside Tondo's, what it made in
regions besides, which Go would give its heap where the value leaves its
function:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | B/op Tondo | B/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| texts/trim read | 13.7 ns | 4.8 ns | 2.86× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 333 | 113 | 66 | 23 |
| texts/trim | 13.7 ns | 5.2 ns | 2.63× ⚠ | 0 +1 in regions | 0 | 0 +80 in regions | 0 | 328 | 115 | 66 | 25 |
| programs/sum lines | 24.28 µs | 15.13 µs | 1.61× ⚠ | 5 ⚠ +1 in regions | 2 | 44.8k +128 in regions | 4.0k | 517.0k | 300.0k | 117.5k | 72.3k |
| ffi/from a C thread 1000 | 66.06 µs | 61.05 µs | 1.08× ⚠ | 2 ⚠ | 0 | 625 | 0 | 1.8k | 516.7k | 4.5k | 164.7k |
| tasks/spawn 1000 | 181.98 µs | 171.91 µs | 1.06× ⚠ | 1952 ⚠ | 2002 | 648.2k | 78.5k | 2.24M | 2.32M | 1.37M | 1.62M |
| ffi/from C threads 8x1000 | 182.58 µs | 174.39 µs | 1.05× ⚠ | 16 ⚠ | 0 | 4.9k | 6 | 13.1k | 4.12M | 34.8k | 1.50M |
| texts/toUpper other | 8.69 µs | 8.30 µs | 1.05× ⚠ | 1 +1 in regions | 1 | 1.7k +1.7k in regions | 1.8k | 136.3k | 172.4k | 42.2k | 39.8k |
| net/tls echo 1x1000 | 12.81 ms | 12.25 ms | 1.05× ⚠ | 4900 ⚠ +1890 in regions | 3745 | 105.8k +74.5k in regions | 50.7k | 18.55M | 14.27M | 14.47M | 11.77M |
| runtime/mutex 4x1000 | 21.91 µs | 20.97 µs | 1.04× ⚠ | 5 ⚠ +1 in regions | 6 | 400 +32 in regions | 224 | 408.6k | 120.1k | 101.2k | 151.4k |
| floats/sqrt 100000 | 178.87 µs | 174.40 µs | 1.03× ⚠ | 0 | 0 | 0 | 0 | 725.0k | 700.2k | 892.5k | 879.1k |
| texts/chars ascii | 460.4 ns | 457.5 ns | 1.01× ⚠ | 0 | 0 | 0 | 0 | 10.0k | 9.9k | 2.2k | 2.2k |
| floats/mandelbrot 64 | 139.19 µs | 138.90 µs | 1.00× ⚠ | 0 | 0 | 0 | 0 | 1.48M | 1.57M | 684.3k | 686.7k |
| processes/true by name | 344.22 µs | 350.17 µs | 0.98× | 469 ⚠ | 47 | 24.3k | 9.4k | 169.9k | 235.5k | 107.7k | 264.8k |
| calls/closure 1000 | 203.5 ns | 207.3 ns | 0.98× | 0 | 0 | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| net/echo 1x1000 | 11.21 ms | 11.52 ms | 0.97× | 900 +1890 in regions | 3744 | 28.1k +74.5k in regions | 50.5k | 10.50M | 7.15M | 9.23M | 7.47M |
| processes/true by path | 330.95 µs | 343.13 µs | 0.96× | 459 ⚠ | 34 | 23.3k | 8.3k | 166.2k | 224.8k | 103.6k | 250.3k |
| queries/uniqueBy 10000 | 145.78 µs | 151.37 µs | 0.96× | 0 +40 in regions | 32 | 0 +128.2k in regions | 160.2k | 2.09M | 1.94M | 699.1k | 765.0k |
| arrays/index sum 10000 | 2.06 µs | 2.19 µs | 0.94× | 0 | 0 | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.8k |
| queries/countBy 10000 | 102.52 µs | 110.97 µs | 0.92× | 0 +7 in regions | 2 | 0 +352 in regions | 256 | 1.75M | 1.55M | 491.0k | 532.8k |
| texts/chars other | 1.01 µs | 1.11 µs | 0.92× | 0 | 0 | 0 | 0 | 12.8k | 28.2k | 5.0k | 5.4k |
| ffi/waits from a C thread 100 | 958.00 µs | 1.05 ms | 0.92× | 109 | 200 | 5.3k | 12.5k | 236.7k | 2.31M | 473.6k | 2.22M |
| programs/count words | 10.17 µs | 11.19 µs | 0.91× | 0 +7 in regions | 3 | 0 +25.1k in regions | 9.8k | 189.6k | 185.6k | 48.1k | 52.5k |
| strings/builder 100 | 713.0 ns | 811.5 ns | 0.88× | 0 +5 in regions | 8 | 0 +1.2k in regions | 1.9k | 15.5k | 13.4k | 3.4k | 3.8k |
| json/decode struct | 1.07 µs | 1.25 µs | 0.85× | 1 | 4 | 64 | 288 | 16.3k | 21.0k | 5.2k | 6.1k |
| queries/iter filter sumBy 10000 | 5.26 µs | 6.21 µs | 0.85× | 0 +1 in regions | 0 | 0 +48 in regions | 0 | 160.2k | 80.0k | 25.2k | 30.3k |
| texts/parse ints 1000 | 5.59 µs | 6.64 µs | 0.84× | 0 | 0 | 0 | 0 | 133.1k | 144.5k | 27.1k | 31.8k |
| loops/sum 1000000 | 610.32 µs | 735.40 µs | 0.83× | 0 | 0 | 0 | 0 | 9.25M | 15.00M | 3.00M | 3.59M |
| runtime/select 1000 | 119.47 µs | 144.27 µs | 0.83× | 4 ⚠ | 4 | 722 | 528 | 1.31M | 2.09M | 535.9k | 728.9k |
| queries/numbers iter map filter sum 10000 | 5.33 µs | 6.47 µs | 0.82× | 0 +2 in regions | 0 | 0 +96 in regions | 0 | 160.3k | 142.8k | 25.6k | 30.9k |
| loops/collatz 10000 | 750.90 µs | 913.17 µs | 0.82× | 0 | 0 | 0 | 0 | 14.54M | 8.86M | 3.67M | 4.46M |
| queries/maxBy 10000 | 6.70 µs | 8.19 µs | 0.82× | 0 | 0 | 0 | 0 | 145.2k | 150.1k | 32.6k | 39.2k |
| json/encode 100 structs | 24.24 µs | 30.04 µs | 0.81× | 1 +1 in regions | 4 | 1 +7.2k in regions | 16.1k | 407.1k | 442.2k | 115.4k | 144.5k |
| texts/split other | 1.31 µs | 1.63 µs | 0.80× | 0 +1 in regions | 1 | 0 +9.4k in regions | 3.4k | 25.2k | 27.0k | 6.3k | 7.9k |
| http/hello 1x1000 | 31.35 ms | 39.66 ms | 0.79× | 49000 ⚠ +1000 in regions | 69145 | 19.48M +109.4k in regions | 6.18M | 67.00M | 143.21M | 76.00M | 128.95M |
| runtime/join 1 | 249.3 ns | 319.5 ns | 0.78× | 1 | 2 | 40 | 144 | 2.7k | 3.3k | 1.2k | 1.5k |
| http/tls 64x100 | 65.84 ms | 85.24 ms | 0.77× | 378709 ⚠ +6405 in regions | 460149 | 51.90M +701.1k in regions | 51.50M | 583.83M | 1.84G | 913.61M | 1.75G |
| strings/contains 1000 | 12.1 ns | 15.8 ns | 0.77× | 0 | 0 | 0 | 0 | 320 | 394 | 58 | 76 |
| strings/split 100 | 576.2 ns | 788.8 ns | 0.73× | 0 +1 in regions | 1 | 0 +4.4k in regions | 1.8k | 12.0k | 13.8k | 2.8k | 4.2k |
| maps/lookup 10000 ints | 54.24 µs | 74.55 µs | 0.73× | 0 | 0 | 0 | 0 | 970.1k | 1.06M | 258.7k | 356.0k |
| programs/reprice order | 1.18 µs | 1.63 µs | 0.73× | 1 +1 in regions | 9 | 64 +128 in regions | 627 | 16.8k | 20.8k | 5.7k | 7.8k |
| calls/fib 25 | 207.76 µs | 290.64 µs | 0.71× | 0 | 0 | 0 | 0 | 4.81M | 3.52M | 1.01M | 1.41M |
| tasks/channel 10000 | 188.92 µs | 271.17 µs | 0.70× | 2 ⚠ | 2 | 744 | 664 | 3.15M | 4.42M | 906.5k | 1.28M |
| strings/int to text 1000 | 7.18 µs | 10.96 µs | 0.66× | 0 | 900 | 0 | 2.8k | 132.3k | 211.2k | 34.5k | 51.6k |
| programs/sort adults | 160.74 µs | 267.79 µs | 0.60× | 12 +697 in regions | 710 | 75.7k +58.1k in regions | 90.4k | 2.43M | 4.04M | 770.6k | 1.28M |
| queries/joinBy 10000x1000 | 447.44 µs | 755.14 µs | 0.59× | 1045 ⚠ | 1039 | 1.67M | 2.91M | 5.98M | 12.87M | 2.15M | 5.84M |
| texts/join 100 | 386.0 ns | 657.1 ns | 0.59× | 0 +1 in regions | 1 | 0 +1.0k in regions | 576 | 6.8k | 11.5k | 1.9k | 3.1k |
| strings/interpolate | 17.6 ns | 33.9 ns | 0.52× | 0 +1 in regions | 1 | 0 +64 in regions | 32 | 368 | 663 | 83 | 158 |
| arrays/map filter sum 10000 | 15.87 µs | 33.16 µs | 0.48× | 0 +12 in regions | 16 | 0 +142.5k in regions | 165.2k | 323.5k | 483.0k | 76.3k | 204.6k |
| json/parse object | 1.65 µs | 3.62 µs | 0.46× | 14 ⚠ | 36 | 1.7k | 1.5k | 26.1k | 47.8k | 8.0k | 17.8k |
| queries/groupBy 10000 | 245.44 µs | 538.55 µs | 0.46× | 2683 +1 in regions | 4664 | 879.9k +48 in regions | 1.18M | 2.87M | 7.38M | 1.15M | 3.35M |
| texts/replace other | 507.0 ns | 1.14 µs | 0.44× | 0 +1 in regions | 1 | 0 +1.7k in regions | 1.8k | 11.5k | 19.5k | 2.4k | 5.2k |
| queries/numbers map filter sum 10000 | 16.07 µs | 36.53 µs | 0.44× | 0 +14 in regions | 16 | 0 +142.6k in regions | 165.2k | 323.5k | 647.5k | 76.8k | 272.1k |
| texts/replace ascii | 558.4 ns | 1.27 µs | 0.44× | 0 +1 in regions | 1 | 0 +1.0k in regions | 1.0k | 12.9k | 24.6k | 2.7k | 6.1k |
| queries/averageBy 10000 | 10.49 µs | 24.16 µs | 0.43× | 0 | 0 | 0 | 0 | 200.2k | 90.2k | 50.1k | 120.8k |
| queries/toSet 10000 | 197.70 µs | 499.94 µs | 0.40× | 0 +42 in regions | 81 | 0 +1.00M in regions | 577.8k | 2.41M | 5.79M | 951.9k | 2.55M |
| json/parse 1000 numbers | 49.72 µs | 129.32 µs | 0.38× | 9 | 2015 | 32.2k | 54.2k | 956.1k | 2.12M | 235.8k | 617.5k |
| queries/filter sumBy 10000 | 55.08 µs | 145.02 µs | 0.38× | 0 +14 in regions | 16 | 0 +512.3k in regions | 727.7k | 528.2k | 2.45M | 265.4k | 1.26M |
| http/json 64x100 | 30.58 ms | 80.56 ms | 0.38× | 454972 ⚠ +8709 in regions | 700980 | 136.31M +609.1k in regions | 92.31M | 629.18M | 3.53G | 1.72G | 6.06G |
| queries/sortedBy 10000 | 1.33 ms | 3.91 ms | 0.34× | 3 ⚠ | 2 | 1.07M | 392.0k | 15.53M | 31.48M | 6.35M | 19.39M |
| ffi/callback 1000 | 9.82 µs | 29.94 µs | 0.33× | 0 | 0 | 0 | 0 | 140.2k | 497.4k | 47.1k | 147.8k |
| texts/contains other | 152.2 ns | 478.8 ns | 0.32× | 0 | 0 | 0 | 0 | 3.1k | 8.2k | 733 | 2.3k |
| programs/build tree | 42.57 µs | 134.30 µs | 0.32× | 0 +1365 in regions | 4095 | 0 +128.0k in regions | 298.6k | 863.0k | 2.20M | 204.6k | 744.2k |
| maps/insert 1000 strings | 30.50 µs | 100.06 µs | 0.30× | 0 +518 in regions | 1920 | 0 +102.9k in regions | 121.0k | 486.6k | 1.30M | 147.4k | 462.0k |
| structs/users 1000 | 11.46 µs | 39.27 µs | 0.29× | 0 +937 in regions | 1910 | 0 +59.5k in regions | 72.8k | 241.4k | 665.9k | 54.8k | 176.6k |
| maps/insert 10000 ints | 139.48 µs | 503.73 µs | 0.28× | 0 +42 in regions | 79 | 0 +1.25M in regions | 577.6k | 2.32M | 5.33M | 663.4k | 2.36M |
| http/hello 64x100 | 18.98 ms | 73.78 ms | 0.26× | 314008 ⚠ +6405 in regions | 445689 | 124.67M +701.1k in regions | 43.12M | 425.60M | 3.11G | 1.04G | 5.28G |
| queries/keyedBy 10000 | 179.80 µs | 699.49 µs | 0.26× | 0 +43 in regions | 81 | 0 +2.00M in regions | 1.75M | 2.76M | 9.33M | 860.4k | 4.34M |
| regex/match 1000 | 35.78 µs | 147.76 µs | 0.24× | 0 | 0 | 0 | 0 | 499.6k | 2.99M | 173.3k | 702.5k |
| floats/add 100000 | 17.79 µs | 96.64 µs | 0.18× | 1 | 1 | 781.3k | 784.0k | 280.9k | 1.76M | 85.4k | 704.7k |
| arrays/sort 10000 | 63.88 µs | 363.84 µs | 0.18× | 0 +1 in regions | 1 | 0 +78.2k in regions | 80.0k | 1.32M | 2.41M | 308.2k | 1.72M |
| structs/points 1000 | 1.32 µs | 7.52 µs | 0.18× | 0 +1 in regions | 12 | 0 +15.7k in regions | 49.2k | 30.3k | 79.1k | 6.3k | 47.6k |
| tensors/matmul 64 | 81.30 µs | 492.18 µs | 0.17× | 0 +2 in regions | 1 | 0 +32.1k in regions | 32.0k | 1.59M | 4.30M | 396.6k | 2.41M |
| arrays/push 1000 | 527.2 ns | 3.54 µs | 0.15× | 0 +1 in regions | 12 | 0 +7.8k in regions | 24.6k | 12.8k | 40.2k | 2.5k | 23.4k |
| tensors/broadcast 256 | 10.29 µs | 72.73 µs | 0.14× | 1 ⚠ +1 in regions | 1 | 512.0k +48 in regions | 512.0k | 190.2k | 1.44M | 49.7k | 566.3k |
| floats/dot 100000 | 16.31 µs | 242.62 µs | 0.07× | 0 | 0 | 0 | 0 | 262.6k | 999.5k | 78.7k | 1.20M |
| floats/sum 100000 | 7.89 µs | 243.65 µs | 0.03× | 0 | 0 | 0 | 0 | 118.8k | 600.4k | 38.3k | 1.20M |
| texts/toUpper ascii | 53.4 ns | 2.40 µs | 0.02× | 0 +1 in regions | 1 | 0 +1.1k in regions | 1.1k | 1.4k | 51.6k | 257 | 11.4k |

12 of 77 benchmarks are slower than Go; 18 allocate more; 18 run more instructions than Go.

| Server | p50 Tondo | p50 Go | p99 Tondo | p99 Go |
|---|---:|---:|---:|---:|
| http/hello 20000/s | 54 µs | 286 µs | 86 µs | 1415 µs |
| http/json 20000/s | 77 µs | 212 µs | 125 µs | 1413 µs |
| http/tls hello 20000/s | 98 µs ⚠ | 90 µs | 161 µs | 1604 µs |

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/uniqueBy 10000 | 145.78 µs | 61.37 µs | 2.38× ⚠ | 0 | 65.2k |
| queries/countBy 10000 | 102.52 µs | 82.86 µs | 1.24× ⚠ | 0 | 1.1k |
| queries/iter filter sumBy 10000 | 5.26 µs | 69.78 µs | 0.08× | 0 | 96 |
| queries/numbers iter map filter sum 10000 | 5.33 µs | 24.21 µs | 0.22× | 0 | 128 |
| queries/maxBy 10000 | 6.70 µs | 16.26 µs | 0.41× | 0 | 40 |
| queries/joinBy 10000x1000 | 447.44 µs | 134.43 µs | 3.33× ⚠ | 1.67M | 102.3k |
| queries/groupBy 10000 | 245.44 µs | 277.37 µs | 0.88× | 879.9k | 583.9k |
| queries/numbers map filter sum 10000 | 16.07 µs | 11.36 µs | 1.41× ⚠ | 0 | 51.7k |
| queries/averageBy 10000 | 10.49 µs | 25.70 µs | 0.41× | 0 | 24 |
| queries/toSet 10000 | 197.70 µs | 50.18 µs | 3.94× ⚠ | 0 | 158.0k |
| queries/filter sumBy 10000 | 55.08 µs | 48.37 µs | 1.14× ⚠ | 0 | 39.0k |
| queries/sortedBy 10000 | 1.33 ms | 520.86 µs | 2.55× ⚠ | 1.07M | 234.7k |
| queries/keyedBy 10000 | 179.80 µs | 76.76 µs | 2.34× ⚠ | 0 | 276.4k |

The rows still behind Go, and why:

- `texts/trim`, 2.63×, and `texts/trim read`, 2.86×: trim reads ASCII a
  byte at a time and gives a text with nothing to trim back as it is
  (a2d299d7), but a trimmed text is still a new one holding its bytes,
  where Go's is a view of the old: a region takes it, which costs the heap
  nothing, but the call and the copy remain, 328 and 333 instructions
  against Go's 115 and 113, of which the copy and its region are about
  half. `texts/trim read` measures the commoner case, a trimmed text only
  read, `padded.trim().byteLength()` against `len(strings.TrimSpace(padded))`.
  A view of the text would close both; lending a part only read was built,
  measured and set aside, as `docs/decisions.md` says. What is left to try
  is in docs/pending.md: `trim` in Tondo measured again, the inliner
  weighing a call by what inlining buys, and a region for the copy of a
  function over its budget.
- `programs/sum lines`, 1.61×, from 2.34×: piece 1's releases dropped
  after a move took 23% of its instructions, and it runs 517.0 thousand
  against Go's 300.0. Each line `readLine` gives is a text of its own, a
  thousand of them in the regions of the loop's turns, where Go's scanner
  gives a view of its buffer: 5 allocations against Go's 2.
- `ffi/*`: within 8% of Go. A thread of C new to the runtime costs it the
  records it keeps for a thread, and each call a task's record in the
  thread's block.
- `tasks/spawn`, 6%: each task's record with its join, below; and
  `texts/toUpper other`, 5%, and `net/tls echo`, 5%, which makes about one
  more allocation a round trip than Go, and `http/tls` a few more a
  request: rustls 0.23 makes a buffer for each record it encrypts and
  copies each one it reads into another, in its buffered API and its
  unbuffered one alike.
- `runtime/mutex`, 1.04×, `floats/sqrt`, 1.03×, and `floats/mandelbrot`,
  1.00×, were under Go's time in the table before, with the same
  instructions on both sides now: mandelbrot's cycles are level with Go's,
  684.3 thousand against 686.7, with Go at 4.94 GHz in this run where it
  was at 4.62 then; sqrt runs 1.5% more cycles than Go; mutex runs four
  threads.
- `texts/chars ascii`, 1.01×, is behind Go's loop in a slow placement:
  Go's row takes 457.5 ns here with the instructions it had at 306.2 ns
  in the table before, where it took 1.5 thousand cycles, not 2.2. Against
  that time Tondo's 460.4 ns is 1.50×. Its 10.0 thousand instructions are
  those of main before piece 1: laying the decoding's way out apart took
  it to 8.9 thousand, and since dead values go, regalloc2 copies the
  loop's count on its way back, as the count needs a register that
  survives the exit's call of the runtime. That copy is the first piece of
  step 3 of docs/pending.md.

Seven rows that were behind in the table before are at or under Go's time
now. Five are piece 1 of pull request 51's, by its measure against main:
`queries/numbers iter map filter sum`, 2.18× to 0.82×, and `queries/iter
filter sumBy`, 2.04× to 0.85×, with 41.7% and 30.3% fewer instructions;
`texts/chars other`, 1.57× to 0.92×, 61.9% fewer; `queries/countBy`, 1.16×
to 0.92×, 19.7% fewer; and `programs/count words`, 1.07× to 0.91×, 12.5%
fewer. docs/decisions.md says which of its changes does what, where one
was measured alone. `texts/parse ints`, 1.16× to 0.84×, is the parser that
reads a short text's digits from its word in a register, all at once,
which came after the table before; `net/echo`, 0.97×, is within what it
moves from one run to the next.

The two rows that start a program, `processes/true by name` and `by path`,
at 0.98× and 0.96×, allocate ten and thirteen times as often as Go: a
start copies the whole environment, about seven allocations and three
thousand instructions a variable, 90 allocations with 2 variables, 469
with this table's 55 and 1,719 with the 225 of a session's shell, which is
why the table names its environment.

The table before this one, at 25ff8924, gave 60 and 59 rows of 75 at or
under Go's time in two runs; this one gives 65 of 77 in one run, the two
rows that start a program new since. Go's side is as it was then: its
times have a median of 1.004 of those, at 4.88 GHz against 4.87, and its
instructions are within 1% on 59 of 75 rows; three of its rows moved more
than 10% with the same instructions, `texts/chars ascii` 49% slower,
`calls/closure` 50% faster and `floats/add` 15% slower, as Go's code lay
elsewhere. Two runs in an environment of six variables, the same day,
gave 66 and 65.

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
| http/hello 20000/s | 54 µs | 286 µs | 86 µs | 1415 µs |
| http/json 20000/s | 77 µs | 212 µs | 125 µs | 1413 µs |
| http/tls hello 20000/s | 98 µs ⚠ | 90 µs | 161 µs | 1604 µs |

Tondo's median is behind Go's on `http/tls hello` alone, and its 99th
percentile is about a tenth of Go's on every server; why the median of
TLS is behind is still to be found.

The rows that LINQ says something of, against C# (`bench/csharp`, measured
as `go test -bench` measures, its bytes from the allocator and no count of
allocations):

| Benchmark | Tondo | C# | Tondo/C# | B/op Tondo | B/op C# |
|---|---:|---:|---:|---:|---:|
| queries/uniqueBy 10000 | 145.78 µs | 61.37 µs | 2.38× ⚠ | 0 | 65.2k |
| queries/countBy 10000 | 102.52 µs | 82.86 µs | 1.24× ⚠ | 0 | 1.1k |
| queries/iter filter sumBy 10000 | 5.26 µs | 69.78 µs | 0.08× | 0 | 96 |
| queries/numbers iter map filter sum 10000 | 5.33 µs | 24.21 µs | 0.22× | 0 | 128 |
| queries/maxBy 10000 | 6.70 µs | 16.26 µs | 0.41× | 0 | 40 |
| queries/joinBy 10000x1000 | 447.44 µs | 134.43 µs | 3.33× ⚠ | 1.67M | 102.3k |
| queries/groupBy 10000 | 245.44 µs | 277.37 µs | 0.88× | 879.9k | 583.9k |
| queries/numbers map filter sum 10000 | 16.07 µs | 11.36 µs | 1.41× ⚠ | 0 | 51.7k |
| queries/averageBy 10000 | 10.49 µs | 25.70 µs | 0.41× | 0 | 24 |
| queries/toSet 10000 | 197.70 µs | 50.18 µs | 3.94× ⚠ | 0 | 158.0k |
| queries/filter sumBy 10000 | 55.08 µs | 48.37 µs | 1.14× ⚠ | 0 | 39.0k |
| queries/sortedBy 10000 | 1.33 ms | 520.86 µs | 2.55× ⚠ | 1.07M | 234.7k |
| queries/keyedBy 10000 | 179.80 µs | 76.76 µs | 2.34× ⚠ | 0 | 276.4k |

Tondo is ahead of C# on both lazy chains, thirteen times on the one over
records and four times on the other, on `maxBy`, `averageBy` and
`groupBy`; C# on the rest, 1.1 to 1.4 times on the eager chains, 1.2 times
on `countBy`, and two to four times on sorting, on sets and on most maps. C#'s times move more
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
