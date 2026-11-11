# std.diagnostics

the program's own counters, which `tondo counters` shows next to the runtime's; both, for the program to send where it wants; and the stack of the task that asks.

```tondo
import std.diagnostics

let served = diagnostics.counter("app.requests")
let queued = diagnostics.gauge("app.queue")
served.add()
queued.set(3)
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.diagnostics` shows the same in a terminal.

- [Functions](#functions): [`counter`](#counter), [`gauge`](#gauge), [`metrics`](#metrics), [`stack`](#stack)
- [Counter](#counter-1): [`Counter.add`](#counteradd), [`Counter.value`](#countervalue)
- [Gauge](#gauge-1): [`Gauge.set`](#gaugeset), [`Gauge.add`](#gaugeadd), [`Gauge.value`](#gaugevalue)
- [Metric](#metric)
- [Frame](#frame)

### Functions

#### counter

```tondo
pub fn counter(name: String): Counter
```

The counter named `name`: asking for the name again gives the same one,
from any task.

#### gauge

```tondo
pub fn gauge(name: String): Gauge
```

The gauge named `name`: asking for the name again gives the same one,
from any task.

#### metrics

```tondo
pub fn metrics(): Array[Metric]
```

Each counter of the runtime and of the program, as `tondo counters` shows
them now: `memory.held`, `tasks.live`, `program.` and a name of the
program's. A program that calls it counts from its start.

#### stack

```tondo
pub fn stack(): Array[Frame]
```

The frames of the task that calls it, from the function that calls
`stack` outwards, with those inlined, as a panic's report shows them.

### Counter

```tondo
pub type Counter = {
    // private fields
}
```

A count that only grows, such as the requests served; `tondo counters`
shows it with its rate per second.

```tondo
import std.diagnostics

// `tondo counters <program>` shows it as `program.orders.placed`.
let placed = diagnostics.counter("orders.placed")
placed.add()
placed.add(2)
```

#### Counter.add

```tondo
pub fn Counter.add(self, amount: Int = 1)
```

Adds `amount`, which is not negative: a count that goes down is a
`Gauge`.

#### Counter.value

```tondo
pub fn Counter.value(self): Int
```

The count so far.

### Gauge

```tondo
pub type Gauge = {
    // private fields
}
```

A level that goes up and down, such as the length of a queue.

```tondo
import std.diagnostics

let queued = diagnostics.gauge("jobs.queued")
queued.set(10)
queued.add(-1)
```

#### Gauge.set

```tondo
pub fn Gauge.set(self, value: Int)
```

Sets the level.

#### Gauge.add

```tondo
pub fn Gauge.add(self, amount: Int)
```

Adds `amount`, which may be negative.

#### Gauge.value

```tondo
pub fn Gauge.value(self): Int
```

The level now.

### Metric

```tondo
pub type Metric = {
    name: String
    value: Int
    unit: String
    gauge: Bool
}
```

A counter as `tondo counters` shows it: its name, value and unit,
`bytes`, `us` or empty, and whether it is a level or a total that only
grows.

```tondo
import std.console
import std.diagnostics

for metric in diagnostics.metrics().filter((m) { m.name.startsWith("tasks.") }) {
    console.println("{metric.name} = {metric.value}{metric.unit}")
}
```

### Frame

```tondo
pub type Frame = {
    function: String
    file: String
    line: Int
    column: Int
}
```

A frame of a stack: the function and where in it.

```tondo
import std.console
import std.diagnostics

for frame in diagnostics.stack() {
    console.println("{frame.function} at {frame.file}:{frame.line}")
}
```

Implements `Display`.
