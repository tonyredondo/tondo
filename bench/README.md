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
Now, at 23bfd25:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go |
|---|---:|---:|---:|---:|---:|
| json/decode struct | 3.05 µs | 1.28 µs | 2.39× ⚠ | 52 | 4 |
| strings/builder 100 | 1.40 µs | 748.8 ns | 1.86× ⚠ | 6 | 8 |
| strings/split 100 | 1.30 µs | 734.2 ns | 1.77× ⚠ | 101 | 1 |
| tasks/spawn 1000 | 269.30 µs | 170.55 µs | 1.58× ⚠ | 2207 | 2002 |
| calls/fib 25 | 387.24 µs | 286.90 µs | 1.35× ⚠ | 0 | 0 |
| loops/sum 1000000 | 928.65 µs | 717.74 µs | 1.29× ⚠ | 0 | 0 |
| calls/closure 1000 | 243.1 ns | 205.5 ns | 1.18× ⚠ | 1 | 0 |
| loops/collatz 10000 | 930.43 µs | 836.94 µs | 1.11× ⚠ | 0 | 0 |
| strings/int to text 1000 | 12.29 µs | 11.35 µs | 1.08× ⚠ | 900 | 900 |
| floats/sqrt 100000 | 181.33 µs | 169.90 µs | 1.07× ⚠ | 0 | 0 |
| json/encode 100 structs | 31.25 µs | 29.42 µs | 1.06× ⚠ | 114 | 4 |
| floats/mandelbrot 64 | 137.90 µs | 138.56 µs | 1.00× | 0 | 0 |
| arrays/index sum 10000 | 2.06 µs | 2.17 µs | 0.95× | 0 | 0 |
| json/parse object | 3.38 µs | 3.58 µs | 0.94× | 65 | 36 |
| strings/interpolate | 28.6 ns | 32.6 ns | 0.88× | 1 | 1 |
| strings/contains 1000 | 12.3 ns | 15.7 ns | 0.78× | 0 | 0 |
| tasks/channel 10000 | 209.76 µs | 279.39 µs | 0.75× | 316 | 2 |
| json/parse 1000 numbers | 95.34 µs | 127.75 µs | 0.75× | 1913 | 2015 |
| maps/lookup 10000 ints | 52.24 µs | 75.07 µs | 0.70× | 0 | 0 |
| arrays/map filter sum 10000 | 19.93 µs | 30.32 µs | 0.66× | 28 | 16 |
| structs/users 1000 | 20.68 µs | 37.31 µs | 0.55× | 1010 | 1910 |
| maps/insert 1000 strings | 40.54 µs | 98.66 µs | 0.41× | 1030 | 1920 |
| arrays/push 1000 | 1.03 µs | 3.04 µs | 0.34× | 10 | 12 |
| structs/points 1000 | 2.16 µs | 6.45 µs | 0.33× | 10 | 12 |
| maps/insert 10000 ints | 131.79 µs | 485.65 µs | 0.27× | 42 | 79 |
| regex/match 1000 | 40.90 µs | 152.44 µs | 0.27× | 0 | 0 |
| arrays/sort 10000 | 65.88 µs | 354.48 µs | 0.19× | 1 | 1 |

11 of 27 benchmarks are slower than Go.

`tasks/spawn` takes one of two times a run, for Tondo and Go alike: the
5950X's two sets of eight cores share no cache, and a run whose threads
land on both pays for each task that crosses. Tondo takes 144 or 265 µs,
Go 171 or 265 µs; the table caught Tondo's slower one.

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
