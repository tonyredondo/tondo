# std.testing

What a test tells the runner: a line for its report, that it is skipped, a
snapshot to compare with, a clock that does not wait, a benchmark or a
search for inputs. It can be imported only in a file of tests, one whose
name ends in `_test.to` or one under `tests/`.

```tondo test
import std.testing
import std.time

fn retry(attempts: Int): Int {
    var tries = 0
    for tries < attempts {
        time.sleep(time.seconds(30))
        tries += 1
    }
    tries
}

test addsNumbers {
    testing.log("adding")
    testing.tags(["area": "arithmetic"])
    assert(1 + 1 == 2)
}

test retriesWithoutWaiting {
    testing.withVirtualTime((clock) {
        let job = spawn retry(3)
        clock.advance(time.seconds(90))
        assert(job.wait() == 3)
    })
}

test isMeasured {
    testing.bench("sum", () { [1, 2, 3].sum() })?
}
```

`test`, `suite` and `assert` are the language's and need no import.
[Testing](../language/16-testing.md) explains every kind of test with
examples, and [the toolchain](../toolchain.md) what `tondo test` does with
them.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.testing` shows the same in a terminal.

- [Functions](#functions): [`log`](#log), [`tags`](#tags), [`failNow`](#failnow), [`skip`](#skip), [`attach`](#attach), [`snapshot`](#snapshot), [`withVirtualTime`](#withvirtualtime), [`bench`](#bench), [`keep`](#keep)
- [VirtualTime](#virtualtime): [`VirtualTime.settle`](#virtualtimesettle), [`VirtualTime.advance`](#virtualtimeadvance)

### Functions

#### log

```tondo
pub fn log(message: String)
```

A line in the test's report.

#### tags

```tondo
pub fn tags(values: Map[String, String])
```

Adds the pairs to the test's tags in its report; a key it has already
with another value panics, and none is added.

#### failNow

```tondo
pub fn failNow(message: String): Never
```

Fails the test now.

#### skip

```tondo
pub fn skip(reason: String): Never
```

Ends the test now as skipped, with the reason in its report.

#### attach

```tondo
pub fn attach(name: String, mediaType: String, data: Array[Byte])
```

Adds a file to the test's report: its name, media type and content.

#### snapshot

```tondo
pub fn snapshot[T: serialization.Encode](
    name: String,
    value: T,
    ignore: Array[String] = [],
    matches: Map[String, String] = [:],
    replace: Map[String, String] = [:],
)
```

Compares the value's JSON with the file of the snapshot `name`;
`ignore` drops paths, `matches` checks them against patterns and
`replace` substitutes them.

#### withVirtualTime

```tondo
pub fn withVirtualTime[E, F: Send + CallOnce[fn(VirtualTime): !E]](body: F): !E
```

Runs the body with a virtual clock that only moves when told.

#### bench

```tondo
pub fn bench[T, E, F: CallMut[fn(): T ! E]](name: String, body: F): !E
```

Makes the test a benchmark, called in its body (testing 10.4): runs the
closure once, or with `tondo test --bench` measures how long a run
takes and what it allocates; keeps what the closure gives. The call
fails as the closure does.

#### keep

```tondo
pub fn keep[T](value: T)
```

Uses the value, so that the optimizer keeps the work that made it.

### VirtualTime

#### VirtualTime.settle

```tondo
pub fn VirtualTime.settle(self)
```

Lets every task that is ready run until all wait.

#### VirtualTime.advance

```tondo
pub fn VirtualTime.advance(self, duration: time.Duration)
```

Moves the virtual clock forward by `duration`, running every task due
on the way until all wait; a negative duration panics.
