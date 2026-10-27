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

The tables were measured on an AMD Ryzen 9 5950X, Linux, with Go 1.27.
Now, at 745845c:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go |
|---|---:|---:|---:|---:|---:|
| json/decode struct | 2.98 µs | 1.30 µs | 2.29× ⚠ | 52 | 4 |
| calls/closure 1000 | 422.1 ns | 207.5 ns | 2.03× ⚠ | 1 | 0 |
| strings/builder 100 | 1.42 µs | 780.8 ns | 1.82× ⚠ | 6 | 8 |
| strings/split 100 | 1.31 µs | 743.3 ns | 1.77× ⚠ | 101 | 1 |
| loops/sum 1000000 | 1.05 ms | 721.86 µs | 1.45× ⚠ | 0 | 0 |
| calls/fib 25 | 389.61 µs | 280.84 µs | 1.39× ⚠ | 0 | 0 |
| loops/collatz 10000 | 1.10 ms | 839.34 µs | 1.31× ⚠ | 0 | 0 |
| floats/sqrt 100000 | 185.56 µs | 170.32 µs | 1.09× ⚠ | 0 | 0 |
| strings/int to text 1000 | 12.42 µs | 11.42 µs | 1.09× ⚠ | 900 | 900 |
| floats/mandelbrot 64 | 151.93 µs | 140.40 µs | 1.08× ⚠ | 0 | 0 |
| json/encode 100 structs | 30.85 µs | 29.55 µs | 1.04× ⚠ | 114 | 4 |
| json/parse object | 3.35 µs | 3.57 µs | 0.94× | 65 | 36 |
| arrays/index sum 10000 | 2.06 µs | 2.19 µs | 0.94× | 0 | 0 |
| strings/interpolate | 28.5 ns | 33.2 ns | 0.86× | 1 | 1 |
| tasks/spawn 1000 | 144.82 µs | 173.64 µs | 0.83× | 2251 | 2002 |
| strings/contains 1000 | 12.5 ns | 15.7 ns | 0.80× | 0 | 0 |
| json/parse 1000 numbers | 90.98 µs | 126.15 µs | 0.72× | 1913 | 2015 |
| tasks/channel 10000 | 201.32 µs | 286.74 µs | 0.70× | 316 | 2 |
| maps/lookup 10000 ints | 51.09 µs | 74.22 µs | 0.69× | 0 | 0 |
| arrays/map filter sum 10000 | 20.64 µs | 30.71 µs | 0.67× | 28 | 16 |
| structs/users 1000 | 21.34 µs | 38.66 µs | 0.55× | 1010 | 1910 |
| maps/insert 1000 strings | 39.78 µs | 100.29 µs | 0.40× | 1030 | 1920 |
| structs/points 1000 | 2.37 µs | 6.81 µs | 0.35× | 10 | 12 |
| arrays/push 1000 | 940.2 ns | 3.16 µs | 0.30× | 10 | 12 |
| maps/insert 10000 ints | 134.75 µs | 485.87 µs | 0.28× | 42 | 79 |
| regex/match 1000 | 42.58 µs | 153.60 µs | 0.28× | 0 | 0 |
| arrays/sort 10000 | 66.37 µs | 358.64 µs | 0.19× | 1 | 1 |

11 of 27 benchmarks are slower than Go.

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
