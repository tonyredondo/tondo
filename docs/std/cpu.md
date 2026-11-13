# std.cpu

What the processor the program runs on has beyond the level it was built for (toolchain 7.8). A test of a feature the level has, or one of the other architecture, is known when compiling: the code under `if cpu.has(...)` that cannot run is left out.

```tondo
import std.console
import std.cpu

if cpu.has(cpu.Feature.Avx2) {
    console.println("AVX2")
} else {
    console.println("no AVX2")
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.cpu` shows the same in a terminal.

- [Functions](#functions): [`has`](#has), [`features`](#features)
- [Feature](#feature)

### Functions

#### has

```tondo
pub fn has(feature: Feature): Bool
```

Whether the processor the program runs on has it. With a variant written
in the call, a feature of the build's level is `true` and one of the
other architecture `false`, with no test; any other reads what the
runtime found when the program started.

```tondo
import std.console
import std.cpu

console.println("{cpu.has(cpu.Feature.Sse42)}")
```

#### features

```tondo
pub fn features(): Array[Feature]
```

Everything of `Feature` that it has, in the order of `Feature`.

```tondo
import std.console
import std.cpu

console.println("{cpu.features().contains(cpu.Feature.Sse42)}")
```

### Feature

```tondo
pub enum Feature {
    Sse3
    Ssse3
    Sse41
    Sse42
    Popcnt
    Avx
    Avx2
    Fma
    Bmi1
    Bmi2
    Lzcnt
    Avx512F
    Avx512Dq
    Avx512Vl
    Avx512Bw
    Pclmulqdq
    Sha
    Neon
    Lse
    Crc32
    Pmull
    Sha2
    Aes
}
```

What a processor may have, each a variant of its own: those of x64, then
those of arm64, then `Aes`, which both have.

```tondo
import std.console
import std.cpu

console.println("{cpu.Feature.Sse42}")
```
