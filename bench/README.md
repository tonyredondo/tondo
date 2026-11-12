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

The tables were measured on an AMD Ryzen 9 5950X, Linux, with Go 1.27.
Now, on 2026-11-12 at ab5eac89:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| loops/collatz 10000 | 905.83 µs | 845.91 µs | 1.07× ⚠ | 0 | 0 | 15.37M | 8.86M | 4.40M | 4.15M |
| tasks/spawn 1000 | 179.61 µs | 171.83 µs | 1.05× ⚠ | 1952 | 2002 | 2.27M | 2.35M | 1.41M | 1.86M |
| floats/sqrt 100000 | 178.24 µs | 174.21 µs | 1.02× ⚠ | 0 | 0 | 725.0k | 700.0k | 890.7k | 865.0k |
| calls/closure 1000 | 205.8 ns | 207.5 ns | 0.99× | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| strings/split 100 | 752.1 ns | 760.2 ns | 0.99× | 1 | 1 | 16.0k | 13.7k | 3.6k | 3.9k |
| net/tls echo 1x1000 | 12.73 ms | 13.06 ms | 0.98× | 7000 | 3744 | 18.66M | 14.34M | 14.28M | 11.83M |
| floats/mandelbrot 64 | 134.91 µs | 139.15 µs | 0.97× | 0 | 0 | 1.20M | 1.57M | 668.0k | 687.5k |
| arrays/index sum 10000 | 2.09 µs | 2.16 µs | 0.97× | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.6k |
| net/echo 1x1000 | 11.29 ms | 11.85 ms | 0.95× | 3000 | 3744 | 10.60M | 7.14M | 9.10M | 7.45M |
| strings/builder 100 | 758.8 ns | 805.1 ns | 0.94× | 6 | 8 | 16.1k | 13.4k | 3.6k | 3.9k |
| runtime/mutex 4x1000 | 20.43 µs | 21.84 µs | 0.94× | 6 | 6 | 409.0k | 120.6k | 94.7k | 151.8k |
| json/decode struct | 1.20 µs | 1.29 µs | 0.93× | 2 | 4 | 18.1k | 21.1k | 5.8k | 6.3k |
| loops/sum 1000000 | 612.29 µs | 724.40 µs | 0.85× | 0 | 0 | 8.25M | 15.00M | 3.00M | 3.54M |
| runtime/join 1 | 262.2 ns | 315.5 ns | 0.83× | 1 | 2 | 2.8k | 3.3k | 1.2k | 1.4k |
| strings/contains 1000 | 12.9 ns | 15.7 ns | 0.82× | 0 | 0 | 332 | 394 | 62 | 75 |
| calls/fib 25 | 232.38 µs | 290.51 µs | 0.80× | 0 | 0 | 4.81M | 3.52M | 1.13M | 1.41M |
| runtime/select 1000 | 124.60 µs | 159.16 µs | 0.78× | 4 | 4 | 1.34M | 2.09M | 547.7k | 742.2k |
| json/encode 100 structs | 23.77 µs | 30.43 µs | 0.78× | 2 | 4 | 406.6k | 439.3k | 113.3k | 147.1k |
| http/hello 1x1000 | 33.11 ms | 43.64 ms | 0.76× | 67000 | 69197 | 71.47M | 147.69M | 81.95M | 144.98M |
| maps/lookup 10000 ints | 54.45 µs | 74.36 µs | 0.73× | 0 | 0 | 965.1k | 1.06M | 258.4k | 354.1k |
| strings/int to text 1000 | 7.94 µs | 10.89 µs | 0.73× | 0 | 900 | 163.6k | 211.3k | 38.2k | 51.9k |
| http/tls 64x100 | 66.22 ms | 92.73 ms | 0.71× | 493857 | 461065 | 613.89M | 1.82G | 949.54M | 1.79G |
| strings/interpolate | 23.2 ns | 32.7 ns | 0.71× | 1 | 1 | 501 | 665 | 111 | 154 |
| tasks/channel 10000 | 191.76 µs | 273.01 µs | 0.70× | 2 | 2 | 3.17M | 4.42M | 909.8k | 1.38M |
| json/parse object | 2.28 µs | 3.62 µs | 0.63× | 17 | 36 | 34.9k | 47.5k | 10.9k | 17.2k |
| arrays/map filter sum 10000 | 16.91 µs | 30.20 µs | 0.56× | 12 | 16 | 374.0k | 482.1k | 80.7k | 201.8k |
| json/parse 1000 numbers | 74.94 µs | 139.28 µs | 0.54× | 909 | 2015 | 1.38M | 2.12M | 359.1k | 673.9k |
| structs/users 1000 | 19.28 µs | 38.12 µs | 0.51× | 1001 | 1910 | 409.6k | 663.1k | 91.7k | 198.2k |
| http/json 64x100 | 51.11 ms | 110.79 ms | 0.46× | 672536 | 714967 | 752.23M | 3.72G | 2.22G | 6.42G |
| maps/insert 1000 strings | 44.99 µs | 98.29 µs | 0.46× | 1030 | 1920 | 680.1k | 1.30M | 214.4k | 473.4k |
| maps/insert 10000 ints | 135.64 µs | 490.74 µs | 0.28× | 42 | 79 | 2.33M | 5.38M | 639.9k | 2.36M |
| regex/match 1000 | 38.09 µs | 149.22 µs | 0.26× | 0 | 0 | 573.3k | 2.99M | 183.6k | 725.1k |
| structs/points 1000 | 1.57 µs | 6.68 µs | 0.23× | 1 | 12 | 34.6k | 82.0k | 7.5k | 46.9k |
| arrays/sort 10000 | 66.85 µs | 354.46 µs | 0.19× | 1 | 1 | 1.37M | 2.41M | 321.0k | 1.72M |
| http/hello 64x100 | 21.05 ms | 111.75 ms | 0.19× | 429280 | 454933 | 456.66M | 3.09G | 1.18G | 5.74G |
| arrays/push 1000 | 511.4 ns | 3.19 µs | 0.16× | 1 | 12 | 13.0k | 40.4k | 2.4k | 25.5k |

3 of 36 benchmarks are slower than Go; 9 run more instructions than Go.

`tasks/spawn` moves with which cores a run's threads land on: the
5950X's two sets of eight cores share no cache, and Tondo's workers keep to
those that share one (docs/decisions.md). `loops/collatz` took 1.02× Go's
time on the measurement before this one, and runs 1.7 times Go's
instructions. `net/tls echo` makes about three more allocations a round trip
than Go, and `http/tls` a few more a request: rustls 0.23 makes a buffer for
each record it encrypts and copies each one it reads into another, in its
buffered API and its unbuffered one alike.

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
