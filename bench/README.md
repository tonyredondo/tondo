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
Now, on 2026-11-12 at 17eb3213:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go | Ins/op Tondo | Ins/op Go | Cycles/op Tondo | Cycles/op Go |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| runtime/select 1000 | 157.32 µs | 147.09 µs | 1.07× ⚠ | 4 | 4 | 1.67M | 2.10M | 689.9k | 731.7k |
| tasks/spawn 1000 | 178.11 µs | 173.23 µs | 1.03× ⚠ | 1954 | 2002 | 2.27M | 2.31M | 1.40M | 1.63M |
| loops/collatz 10000 | 876.74 µs | 856.23 µs | 1.02× ⚠ | 0 | 0 | 15.36M | 8.86M | 4.22M | 4.13M |
| floats/sqrt 100000 | 180.19 µs | 176.18 µs | 1.02× ⚠ | 0 | 0 | 725.0k | 700.0k | 889.2k | 862.6k |
| net/tls echo 1x1000 | 13.11 ms | 12.91 ms | 1.02× ⚠ | 7000 | 3745 | 18.66M | 14.17M | 14.62M | 11.37M |
| net/echo 1x1000 | 11.48 ms | 11.67 ms | 0.98× | 3000 | 3744 | 10.60M | 7.13M | 9.02M | 7.23M |
| calls/closure 1000 | 205.7 ns | 210.8 ns | 0.98× | 0 | 0 | 3.3k | 5.0k | 1.0k | 1.0k |
| runtime/mutex 4x1000 | 20.84 µs | 21.45 µs | 0.97× | 6 | 6 | 408.9k | 119.2k | 95.5k | 143.8k |
| strings/split 100 | 767.8 ns | 790.6 ns | 0.97× | 2 | 1 | 15.9k | 13.8k | 3.6k | 3.9k |
| floats/mandelbrot 64 | 136.35 µs | 141.41 µs | 0.96× | 0 | 0 | 1.19M | 1.57M | 668.7k | 687.9k |
| arrays/index sum 10000 | 2.09 µs | 2.21 µs | 0.95× | 0 | 0 | 45.0k | 40.0k | 10.0k | 10.5k |
| strings/builder 100 | 775.3 ns | 831.0 ns | 0.93× | 6 | 8 | 16.1k | 13.5k | 3.7k | 4.1k |
| json/decode struct | 1.19 µs | 1.31 µs | 0.90× | 2 | 4 | 18.1k | 21.0k | 5.7k | 6.2k |
| strings/interpolate | 28.5 ns | 33.4 ns | 0.86× | 1 | 1 | 499 | 664 | 134 | 156 |
| loops/sum 1000000 | 619.69 µs | 732.01 µs | 0.85× | 0 | 0 | 8.25M | 15.00M | 3.00M | 3.52M |
| strings/contains 1000 | 13.1 ns | 15.9 ns | 0.82× | 0 | 0 | 330 | 394 | 62 | 76 |
| http/hello 1x1000 | 33.56 ms | 41.25 ms | 0.81× | 68000 | 69184 | 71.31M | 151.93M | 83.53M | 149.62M |
| runtime/join 1 | 259.3 ns | 323.4 ns | 0.80× | 1 | 2 | 2.8k | 3.3k | 1.2k | 1.4k |
| json/encode 100 structs | 23.95 µs | 31.21 µs | 0.77× | 2 | 4 | 406.0k | 443.1k | 113.7k | 148.8k |
| calls/fib 25 | 220.98 µs | 291.22 µs | 0.76× | 0 | 0 | 4.57M | 3.52M | 1.06M | 1.40M |
| strings/int to text 1000 | 8.05 µs | 11.01 µs | 0.73× | 0 | 900 | 163.6k | 211.4k | 38.2k | 52.5k |
| maps/lookup 10000 ints | 54.82 µs | 77.18 µs | 0.71× | 0 | 0 | 965.0k | 1.06M | 257.8k | 355.2k |
| http/tls 64x100 | 62.73 ms | 88.60 ms | 0.71× | 500350 | 460312 | 610.65M | 1.74G | 870.79M | 1.52G |
| tasks/channel 10000 | 192.77 µs | 274.96 µs | 0.70× | 2 | 2 | 3.17M | 4.42M | 907.0k | 1.30M |
| json/parse object | 2.30 µs | 3.67 µs | 0.63× | 17 | 36 | 35.0k | 48.1k | 10.9k | 17.3k |
| arrays/map filter sum 10000 | 17.00 µs | 32.15 µs | 0.53× | 12 | 16 | 373.9k | 495.8k | 81.0k | 209.6k |
| json/parse 1000 numbers | 75.42 µs | 144.81 µs | 0.52× | 909 | 2015 | 1.39M | 2.12M | 358.2k | 670.8k |
| structs/users 1000 | 19.48 µs | 38.95 µs | 0.50× | 1001 | 1910 | 409.3k | 672.2k | 91.9k | 202.2k |
| http/json 64x100 | 51.81 ms | 103.81 ms | 0.50× | 678929 | 713159 | 749.91M | 3.50G | 2.21G | 3.89G |
| maps/insert 1000 strings | 45.47 µs | 101.89 µs | 0.45× | 1030 | 1920 | 680.1k | 1.30M | 215.2k | 471.0k |
| maps/insert 10000 ints | 134.80 µs | 498.58 µs | 0.27× | 42 | 79 | 2.33M | 5.36M | 640.3k | 2.47M |
| regex/match 1000 | 38.47 µs | 151.24 µs | 0.25× | 0 | 0 | 573.3k | 2.99M | 184.1k | 712.3k |
| http/hello 64x100 | 22.30 ms | 87.83 ms | 0.25× | 435560 | 452401 | 455.01M | 3.26G | 1.02G | 6.34G |
| structs/points 1000 | 1.58 µs | 7.06 µs | 0.22× | 1 | 12 | 34.5k | 84.7k | 7.5k | 46.3k |
| arrays/sort 10000 | 67.17 µs | 364.33 µs | 0.18× | 1 | 1 | 1.37M | 2.42M | 321.1k | 1.72M |
| arrays/push 1000 | 515.6 ns | 3.32 µs | 0.16× | 1 | 12 | 13.0k | 42.3k | 2.4k | 22.9k |

5 of 36 benchmarks are slower than Go; 9 run more instructions than Go.

`runtime/select` moves between 146 and 157 µs with where its code lies
(`tondo test --bench --layouts`), and `tasks/spawn` with which cores a
run's threads land on: the 5950X's two sets of eight cores share no cache,
and Tondo's workers keep to those that share one (docs/decisions.md).
`net/tls echo` makes four more allocations a round trip than Go: rustls
encrypts each record into a new buffer, and leaves each one it reads in
another. `strings/split` and `http/tls` make more allocations than Go too.

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
