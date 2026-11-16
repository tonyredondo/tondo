# A profiler of .NET in Tondo

`crates/tondo/tests/dotnet.rs` binds `include/corprof.h` with `tondo bind`,
builds `profiler` as a library for C and runs a console program of .NET
under it: the profiler prints when .NET starts it, each module it loads and
when it ends. The test runs on Linux where `dotnet` is installed.

`include/corprof.h` is the header of .NET's profiling interfaces as
dotnet/runtime generates it, at commit
`0b3ba66a27a82e8a039d32a8d9aa3bd94a4f3e4c` (`src/coreclr/pal/prebuilt/inc/corprof.h`),
under the MIT license in `include/LICENSE.TXT`. The other headers of
`include` stand in for those of Windows that it includes, with what it takes
of them and no more; they were written for this test.

`profiler/src/corprof` is not checked in: the test writes it.
