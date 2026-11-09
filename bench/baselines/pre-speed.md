# The baseline before the speed plan

Measured on 2026-11-09 at c5cfe870, on an AMD Ryzen 9 5950X with 32 threads, Linux, Go 1.27, with
`tondo` built for release (docs/speed.md, A5): after phase A's instruments, the two passes it found
that grew as the square of a program's types, and `--bench` without the watches of the tests.
`pre-speed.json` is what `tondo test --bench --save-baseline` wrote in bench/tondo. As before the
refactoring, one run against it is no gate: a phase keeps the `tondo` of c5cfe870 and runs the two
in turns,

```sh
TONDO=target/release/tondo tondo run bench/compare.to -- --old <the tondo of c5cfe870> --rounds 4 --max 5
TONDO=target/release/tondo tondo run bench/compile.to -- --old <the tondo of c5cfe870> --max 5
```

which fail when a row is more than 5% slower, the runtime's on the median of the rounds and the
compiler's by instructions, or allocates more.

## The runtime, HTTP and sockets against Go

| Benchmark | Tondo | Go | Tondo/Go | Allocs/op Tondo | Allocs/op Go |
|---|---:|---:|---:|---:|---:|
| runtime/mutex 4x1000 | 81.70 µs | 21.23 µs | 3.85× ⚠ | 29 | 6 |
| net/tls echo 1x1000 | 43.19 ms | 13.09 ms | 3.30× ⚠ | 26000 | 3744 |
| http/tls 64x100 | 197.16 ms | 83.42 ms | 2.36× ⚠ | 1231085 | 445315 |
| http/hello 1x1000 | 93.96 ms | 42.66 ms | 2.20× ⚠ | 189000 | 68193 |
| strings/split 100 | 1.43 µs | 795.5 ns | 1.79× ⚠ | 101 | 1 |
| floats/sqrt 100000 | 275.00 µs | 175.95 µs | 1.56× ⚠ | 0 | 0 |
| tasks/spawn 1000 | 341.25 µs | 222.04 µs | 1.54× ⚠ | 2155 | 2002 |
| http/json 64x100 | 163.59 ms | 114.32 ms | 1.43× ⚠ | 1517187 | 712548 |
| strings/int to text 1000 | 14.77 µs | 11.41 µs | 1.29× ⚠ | 900 | 900 |
| http/hello 64x100 | 146.89 ms | 114.52 ms | 1.28× ⚠ | 1203572 | 449397 |
| net/echo 1x1000 | 14.83 ms | 12.08 ms | 1.23× ⚠ | 24000 | 3744 |
| loops/sum 1000000 | 860.44 µs | 762.09 µs | 1.13× ⚠ | 0 | 0 |
| json/decode struct | 1.43 µs | 1.28 µs | 1.11× ⚠ | 10 | 4 |
| tasks/channel 10000 | 293.03 µs | 269.02 µs | 1.09× ⚠ | 316 | 2 |
| runtime/select 1000 | 155.17 µs | 146.03 µs | 1.06× ⚠ | 4355 | 4 |
| floats/mandelbrot 64 | 147.59 µs | 146.19 µs | 1.01× ⚠ | 0 | 0 |
| arrays/index sum 10000 | 2.17 µs | 2.25 µs | 0.96× | 0 | 0 |
| loops/collatz 10000 | 917.68 µs | 955.06 µs | 0.96× | 0 | 0 |
| runtime/join 1 | 307.6 ns | 324.8 ns | 0.95× | 5 | 2 |
| strings/builder 100 | 791.9 ns | 886.2 ns | 0.89× | 6 | 8 |
| strings/contains 1000 | 13.8 ns | 16.4 ns | 0.84× | 0 | 0 |
| json/encode 100 structs | 26.06 µs | 31.96 µs | 0.82× | 11 | 4 |
| strings/interpolate | 25.3 ns | 34.2 ns | 0.74× | 1 | 1 |
| calls/fib 25 | 220.68 µs | 302.54 µs | 0.73× | 0 | 0 |
| maps/lookup 10000 ints | 54.76 µs | 76.71 µs | 0.71× | 0 | 0 |
| arrays/map filter sum 10000 | 23.89 µs | 35.04 µs | 0.68× | 26 | 16 |
| calls/closure 1000 | 273.2 ns | 433.7 ns | 0.63× | 1 | 0 |
| json/parse object | 2.39 µs | 3.92 µs | 0.61× | 37 | 36 |
| structs/users 1000 | 20.91 µs | 40.28 µs | 0.52× | 1009 | 1910 |
| json/parse 1000 numbers | 72.93 µs | 146.43 µs | 0.50× | 914 | 2015 |
| maps/insert 1000 strings | 44.59 µs | 102.36 µs | 0.44× | 1030 | 1920 |
| structs/points 1000 | 2.25 µs | 6.84 µs | 0.33× | 9 | 12 |
| arrays/push 1000 | 1.11 µs | 3.58 µs | 0.31× | 9 | 12 |
| maps/insert 10000 ints | 136.48 µs | 511.47 µs | 0.27× | 42 | 79 |
| regex/match 1000 | 40.61 µs | 154.99 µs | 0.26× | 0 | 0 |
| arrays/sort 10000 | 69.04 µs | 379.48 µs | 0.18× | 1 | 1 |

16 of 36 benchmarks are slower than Go.

## Compiling

By `bench/compile.to`, five runs a row after one that warms its cache; instructions of user code,
by `perf`.

| Program | Command | Median | Instructions |
|---|---|---:|---:|
| hello | check | 28.3 ms | 127.5 M |
| hello | build | 76.7 ms | 621.4 M |
| hello | cold run | 77.1 ms | 625.2 M |
| hello | warm run | 11.5 ms | 4.8 M |
| server | check | 53.8 ms | 353.0 M |
| server | build | 175.2 ms | 2468.1 M |
| server | cold run | 171.7 ms | 2489.6 M |
| server | warm run | 16.2 ms | 22.0 M |
| imports | check | 71.1 ms | 510.2 M |
| imports | build | 121.1 ms | 1058.0 M |
| imports | cold run | 121.4 ms | 1061.7 M |
| imports | warm run | 11.5 ms | 4.8 M |
| generated, 3575 lines | check | 148.2 ms | 1255.6 M |
| generated, 3575 lines | build | 463.5 ms | 17234.7 M |
| generated, 3575 lines | cold run | 467.0 ms | 17241.3 M |
| generated, 3575 lines | warm run | 13.5 ms | 13.9 M |
| generated, 24995 lines | check | 829.7 ms | 6966.2 M |
| generated, 24995 lines | build | 2.61 s | 115672.9 M |
| generated, 24995 lines | cold run | 2.64 s | 115745.4 M |
| generated, 24995 lines | warm run | 22.2 ms | 68.9 M |
| generated, 192785 lines | check | 6.58 s | 52977.9 M |
| generated, 192785 lines | build | 21.03 s | 887280.7 M |
| generated, 192785 lines | cold run | 21.07 s | 887771.1 M |
| generated, 192785 lines | warm run | 96.4 ms | 499.3 M |
| hello | first build | 259.5 ms | 1959.4 M |
