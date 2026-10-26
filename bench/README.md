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
Now, at 94f758e:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go |
|---|---:|---:|---:|---:|---:|
| tasks/spawn 1000 | 722.34 µs | 170.56 µs | 4.24× ⚠ | 4334 | 2002 |
| strings/split 100 | 1.90 µs | 737.8 ns | 2.57× ⚠ | 107 | 1 |
| json/decode struct | 2.99 µs | 1.27 µs | 2.35× ⚠ | 52 | 4 |
| calls/closure 1000 | 422.1 ns | 206.4 ns | 2.04× ⚠ | 1 | 0 |
| arrays/index sum 10000 | 4.14 µs | 2.18 µs | 1.90× ⚠ | 0 | 0 |
| strings/builder 100 | 1.42 µs | 770.4 ns | 1.84× ⚠ | 6 | 8 |
| arrays/sort 10000 | 635.81 µs | 360.62 µs | 1.76× ⚠ | 2 | 1 |
| strings/int to text 1000 | 19.13 µs | 11.43 µs | 1.67× ⚠ | 1000 | 900 |
| calls/fib 25 | 390.40 µs | 288.87 µs | 1.35× ⚠ | 0 | 0 |
| loops/collatz 10000 | 1.07 ms | 837.41 µs | 1.28× ⚠ | 0 | 0 |
| floats/mandelbrot 64 | 173.49 µs | 139.32 µs | 1.25× ⚠ | 0 | 0 |
| loops/sum 1000000 | 857.09 µs | 719.24 µs | 1.19× ⚠ | 0 | 0 |
| arrays/map filter sum 10000 | 33.03 µs | 30.13 µs | 1.10× ⚠ | 28 | 16 |
| floats/sqrt 100000 | 184.95 µs | 171.55 µs | 1.08× ⚠ | 0 | 0 |
| json/encode 100 structs | 31.20 µs | 29.65 µs | 1.05× ⚠ | 114 | 4 |
| json/parse object | 3.43 µs | 3.56 µs | 0.96× | 66 | 36 |
| strings/interpolate | 30.6 ns | 32.8 ns | 0.93× | 1 | 1 |
| json/parse 1000 numbers | 103.43 µs | 124.83 µs | 0.83× | 2013 | 2015 |
| strings/contains 1000 | 12.3 ns | 15.7 ns | 0.78× | 0 | 0 |
| tasks/channel 10000 | 207.49 µs | 282.57 µs | 0.73× | 317 | 2 |
| maps/lookup 10000 ints | 51.15 µs | 73.61 µs | 0.69× | 0 | 0 |
| structs/users 1000 | 24.32 µs | 37.95 µs | 0.64× | 1010 | 1910 |
| maps/insert 1000 strings | 42.37 µs | 99.74 µs | 0.42× | 1030 | 1920 |
| structs/points 1000 | 2.75 µs | 6.77 µs | 0.41× | 10 | 12 |
| maps/insert 10000 ints | 138.54 µs | 482.76 µs | 0.29× | 42 | 79 |
| regex/match 1000 | 42.49 µs | 153.28 µs | 0.28× | 0 | 0 |
| arrays/push 1000 | 820.6 ns | 3.14 µs | 0.26× | 10 | 12 |

15 of 27 benchmarks are slower than Go.

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
