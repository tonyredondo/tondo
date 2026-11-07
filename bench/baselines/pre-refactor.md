# The baseline before the refactoring

Measured on 2026-11-07 at 31bd462, on an AMD Ryzen 9 5950X with 32 threads, Linux, Go 1.27,
with `tondo` built for release (docs/refactor.md, phase 0.7). `pre-refactor.json` is what
`tondo test --bench --save-baseline` wrote in bench/tondo. One run against it is no gate: the same
code measured twice, on the same pinned cores, came out up to 9% slower, as the machine warms, and
`--against` builds the old revision's tests with today's `tondo`, so both sides would run today's
runtime. A phase builds the `tondo` of 31bd462 for release, keeps it, and runs the two in turns:

```sh
TONDO=target/release/tondo tondo run bench/compare.to -- --old <the tondo of 31bd462> --rounds 4 --max 5
```

which fails when a benchmark is more than 5% slower on the median of the rounds, or allocates more.
That `tondo` against itself so: 2.4% at worst, but `runtime/mutex 4x1000`, which four tasks
contend for, moved 11%.

and against Go with `TONDO=$(which tondo) tondo run bench/compare.to -- --time 1`, which gave:

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go |
|---|---:|---:|---:|---:|---:|
| runtime/mutex 4x1000 | 333.91 µs | 21.28 µs | 15.69× ⚠ | 4056 | 6 |
| strings/split 100 | 1.35 µs | 743.0 ns | 1.82× ⚠ | 101 | 1 |
| floats/sqrt 100000 | 262.61 µs | 177.47 µs | 1.48× ⚠ | 0 | 0 |
| runtime/select 1000 | 186.86 µs | 145.81 µs | 1.28× ⚠ | 4355 | 4 |
| calls/closure 1000 | 269.7 ns | 213.7 ns | 1.26× ⚠ | 1 | 0 |
| strings/int to text 1000 | 14.09 µs | 11.50 µs | 1.22× ⚠ | 900 | 900 |
| tasks/channel 10000 | 342.73 µs | 288.48 µs | 1.19× ⚠ | 316 | 2 |
| loops/sum 1000000 | 831.56 µs | 727.34 µs | 1.14× ⚠ | 0 | 0 |
| loops/collatz 10000 | 945.45 µs | 848.36 µs | 1.11× ⚠ | 0 | 0 |
| json/decode struct | 1.39 µs | 1.29 µs | 1.08× ⚠ | 12 | 4 |
| json/encode 100 structs | 32.20 µs | 30.09 µs | 1.07× ⚠ | 115 | 4 |
| runtime/join 1 | 358.4 ns | 335.6 ns | 1.07× ⚠ | 5 | 2 |
| tasks/spawn 1000 | 185.51 µs | 174.31 µs | 1.06× ⚠ | 2192 | 2002 |
| floats/mandelbrot 64 | 140.62 µs | 140.03 µs | 1.00× ⚠ | 0 | 0 |
| arrays/index sum 10000 | 2.09 µs | 2.19 µs | 0.96× | 0 | 0 |
| strings/builder 100 | 726.7 ns | 782.2 ns | 0.93× | 6 | 8 |
| strings/contains 1000 | 13.2 ns | 15.9 ns | 0.83× | 0 | 0 |
| strings/interpolate | 27.2 ns | 33.2 ns | 0.82× | 1 | 1 |
| calls/fib 25 | 234.17 µs | 292.42 µs | 0.80× | 0 | 0 |
| arrays/map filter sum 10000 | 21.80 µs | 30.99 µs | 0.70× | 28 | 16 |
| json/parse object | 2.46 µs | 3.60 µs | 0.68× | 39 | 36 |
| maps/lookup 10000 ints | 52.24 µs | 78.03 µs | 0.67× | 0 | 0 |
| json/parse 1000 numbers | 73.78 µs | 127.01 µs | 0.58× | 916 | 2015 |
| structs/users 1000 | 19.92 µs | 38.34 µs | 0.52× | 1010 | 1910 |
| maps/insert 1000 strings | 47.16 µs | 101.59 µs | 0.46× | 1030 | 1920 |
| structs/points 1000 | 2.24 µs | 6.84 µs | 0.33× | 10 | 12 |
| maps/insert 10000 ints | 134.07 µs | 489.93 µs | 0.27× | 42 | 79 |
| arrays/push 1000 | 796.0 ns | 3.16 µs | 0.25× | 10 | 12 |
| regex/match 1000 | 38.19 µs | 156.50 µs | 0.24× | 0 | 0 |
| arrays/sort 10000 | 66.75 µs | 360.19 µs | 0.19× | 1 | 1 |

14 of 30 benchmarks are slower than Go.

YAML has no Go side: `yaml/encode 100 structs` and `yaml/decode 100 structs` are in
`pre-refactor.json`.

A program that prints a line, built for release: 2,646,664 bytes, and it starts and ends in
1.35 ms, the median of 200 runs (1.52 ms at the 90th percentile).
