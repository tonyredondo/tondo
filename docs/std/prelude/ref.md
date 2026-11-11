# Ref, WeakRef and Contexts

`Ref[T]` gives a value an identity its copies share, `WeakRef[T]` refers to it without keeping it alive, and `Contexts` is every context as it is at one place.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Ref`, `tondo api WeakRef`, `tondo api Contexts` show the same in a terminal.

- [Functions](#functions): [`Ref`](#ref)
- [WeakRef](#weakref): [`WeakRef`](#weakref-1), [`WeakRef.get`](#weakrefget)
- [Contexts](#contexts): [`Contexts.current`](#contextscurrent)

### Functions

#### Ref

```tondo
pub fn Ref(value: T): Ref[T]
```

`Ref(value)`: a new identity holding the value (language 16.10). Its
copies share it, `==` and hashing compare identities rather than
values, and `reference.value` reads the value, which never changes.

### WeakRef

#### WeakRef

```tondo
pub fn WeakRef(reference: Ref[T]): WeakRef[T]
```

`WeakRef(reference)`: a reference to the same object that does not keep
it alive, for caches and graphs (language 16.10).

#### WeakRef.get

```tondo
pub fn WeakRef.get(self): Ref[T]?
```

The reference while its object lives, `none` after; a cycle nothing
else uses dies when the collector finds it.

### Contexts

```tondo
pub type Contexts = {
    // private fields
}
```

The contexts of the running task as they are (language 11.16), which
`with captured` gives back until its block ends, in this task or
another: what a queue keeps with each message, so that the task that
handles it does so with the contexts of the one that sent it.

```tondo
import std.console

context Request = {
    id: String = "-"
}

fn handle(jobs: Receiver[(Contexts, String)]) {
    for (captured, job) in jobs {
        work(captured, job)
    }
}

fn work(captured: Contexts, job: String) {
    // The sender's contexts, for this block, in the worker's task.
    with captured
    console.println("{job} for request {Request.id}")
}

with Request { id: "r-42" }
let captured = Contexts.current()
let (sender, jobs) = channel[(Contexts, String)](4)
sender.send((captured, "resize"))
sender.close()
let worker = spawn handle(jobs)
worker.wait()
```

#### Contexts.current

```tondo
pub fn Contexts.current(): Contexts
```

The contexts where it is called, with every change of the `with`s that
hold there.
