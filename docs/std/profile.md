# std.profile

profiles a program makes of itself, in pprof's format, as `tondo profile` writes them: where its processor time goes, what it allocates and holds, and where its tasks are; and labels that divide them, as by endpoint.

```tondo
import std.console
import std.fs
import std.profile

let cpu = profile.startCpu()
var total = 0
for n in 0..1_000_000 {
    total += n % 7
}
fs.writeBytes("cpu.pb.gz", cpu.stop().pprof())?
console.println("{total}, profiled in cpu.pb.gz")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.profile` shows the same in a terminal.

- [Functions](#functions): [`startCpu`](#startcpu), [`startMemory`](#startmemory), [`memory`](#memory), [`tasks`](#tasks), [`startBlocking`](#startblocking), [`blocking`](#blocking), [`startContention`](#startcontention), [`contention`](#contention), [`withLabels`](#withlabels), [`setLabels`](#setlabels)
- [Profile](#profile): [`Profile.pprof`](#profilepprof), [`Profile.folded`](#profilefolded), [`Profile.since`](#profilesince)
- [CpuProfile](#cpuprofile): [`CpuProfile.stop`](#cpuprofilestop), [`CpuProfile.period`](#cpuprofileperiod)
- [Labels](#labels): [`Labels.restore`](#labelsrestore)

### Functions

#### startCpu

```tondo
pub fn startCpu(every: time.Duration = time.milliseconds(1)): CpuProfile
```

Begins a profile of where the program's threads spend their processor
time, sampled each `every` of it, from 1 ms to 1 s, until `stop`; one at
a time. Datadog's profiler samples each 10 ms.

#### startMemory

```tondo
pub fn startMemory(every: Int = 512 * 1024)
```

Samples what the program allocates from now on, about one allocation
in each `every` bytes, which `memory` reports.

#### memory

```tondo
pub fn memory(): Profile
```

The memory alive now and what the program allocated, by the stack that
allocated it, from the samples since `startMemory`.

#### tasks

```tondo
pub fn tasks(): Profile
```

Where each task of the program is now.

#### startBlocking

```tondo
pub fn startBlocking(every: time.Duration = time.microseconds(100))
```

Begins sampling where tasks wait for others: channels, `select`, joins
and groups, and the locks, semaphores and barriers of `std.sync`; one
wait for each `every` waited on average, and every longer one.

#### blocking

```tondo
pub fn blocking(): Profile
```

How many waits for other tasks and how long, by the stack that waited,
from the samples since `startBlocking`: Go's block profile.

#### startContention

```tondo
pub fn startContention(every: time.Duration = time.microseconds(100))
```

Begins sampling the locks of `std.sync` that tasks waited for, as
`startBlocking` samples waits.

#### contention

```tondo
pub fn contention(): Profile
```

Where locks others waited for were let go, and how long they waited,
from the samples since `startContention`: Go's mutex profile, the code
that holds a lock too long.

#### withLabels

```tondo
pub fn withLabels[T: ~Copy](labels: Map[String, String], body: fn(): T): T
```

Runs `body` with `labels`, which the samples of its processor time carry,
and those of the tasks it starts, so that a profile divides the time by
them; labels already there stay, and those of the same names are
replaced. It gives what `body` gives: a body that can fail gives its
result, to pass on with `?`.

#### setLabels

```tondo
pub fn setLabels(labels: Map[String, String]): Labels
```

Adds `labels` to those of the task, as `withLabels` does but without a
block: until the `Labels` it gives, the ones the task had, are restored.
For a start and an end in different calls, as a span's:
`let previous = profile.setLabels(["span": id])` and
`defer previous.restore()`.

### Profile

```tondo
pub type Profile = {
    // private fields
}
```

A profile: in pprof's format, for a file or a service, and as folded
stacks.

```tondo
import std.fs
import std.profile

profile.startMemory()
let lists = (0..1000).map((n) { [n] }).collect()
fs.writeBytes("memory.pb.gz", profile.memory().pprof())?
_ = lists
```

#### Profile.pprof

```tondo
pub fn Profile.pprof(self): Array[Byte]
```

The profile in pprof's format, gzipped, as `go tool pprof`, Datadog and
`tondo profile show` read it.

#### Profile.folded

```tondo
pub fn Profile.folded(self): String
```

The profile as folded stacks: a line for each stack, its frames from
the outermost joined by `;`, and how much it took.

#### Profile.since

```tondo
pub fn Profile.since(self, earlier: Profile): Profile
```

What happened between `earlier` and this profile, of the same kind: what
adds up from when sampling began, as allocations and waits, less what
`earlier` had, by stack and labels; what is alive now, as the memory in
use, as it is; over the time between them. The profile of a period, as
Datadog and Go's delta profiles send: `memory.since(last)`.

### CpuProfile

```tondo
pub type CpuProfile = {
    // private fields
}
```

A profile of the processor time, which runs until `stop`.

```tondo
import std.console
import std.profile

let cpu = profile.startCpu()
var total = 0
for n in 0..100_000 {
    total += n % 3
}
console.println("{total}")
console.print(cpu.stop().folded())
```

#### CpuProfile.stop

```tondo
pub fn CpuProfile.stop(self): Profile
```

What was sampled since `startCpu`, or since the last `period`.

#### CpuProfile.period

```tondo
pub fn CpuProfile.period(self): Profile
```

What was sampled since `startCpu` or the last `period`, as sampling
goes on: a profiler that sends one each minute calls it each minute.

### Labels

```tondo
pub type Labels = {
    // private fields
}
```

The labels a task had before `setLabels`, which `restore` puts back.

```tondo
import std.profile

// Samples taken until `restore` carry these labels, as `tondo profile show` reads them.
let labels = profile.setLabels(["endpoint": "/users"])
labels.restore()
```

#### Labels.restore

```tondo
pub fn Labels.restore(self)
```

Puts back the labels the task had when `setLabels` gave these.
