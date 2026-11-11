# Join, Group and channels

What `spawn` gives, a group of tasks, and the two ends of a channel.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Join`, `tondo api Group`, `tondo api Sender`, `tondo api Receiver` show the same in a terminal.

- [Join](#join): [`Join.wait`](#joinwait), [`Join.cancel`](#joincancel), [`Join.detach`](#joindetach)
- [Group](#group): [`Group`](#group-1), [`Group.add`](#groupadd), [`Group.wait`](#groupwait), [`Group.next`](#groupnext), [`Group.cancel`](#groupcancel), [`Group.length`](#grouplength)
- [Sender](#sender): [`Sender.send`](#sendersend), [`Sender.close`](#senderclose), [`Sender.isClosed`](#senderisclosed), [`Sender.isAbandoned`](#senderisabandoned), [`Sender.copy`](#sendercopy)
- [Receiver](#receiver): [`Receiver.receive`](#receiverreceive), [`Receiver.isClosed`](#receiverisclosed)

### Join

#### Join.wait

```tondo
pub fn Join.wait(self): T
```

`spawn call(arguments)` starts a task (language 11.11).

#### Join.cancel

```tondo
pub fn Join.cancel(self)
```

Cancels the task and waits until it ends, its cleanup run (language
11.15).

#### Join.detach

```tondo
pub fn Join.detach(self)
```

Lets the task run on without anyone waiting.

### Group

#### Group

```tondo
pub fn Group[T](): Group[T]
```

An empty group of tasks whose results are `T` (language 11.12).

#### Group.add

```tondo
pub fn Group.add(var self, job: Join[T])
```

Adds a task to the group.

#### Group.wait

```tondo
pub fn Group.wait(self): Array[T]
```

Every result, in order; with `T = R ! E`, the first error cancels the rest.

#### Group.next

```tondo
pub fn Group.next(var self): T?
```

The next task to finish, or `none` when the group is empty.

#### Group.cancel

```tondo
pub fn Group.cancel(self)
```

Cancels every task of the group and waits until they end.

#### Group.length

```tondo
pub fn Group.length(self): Int
```

The number of tasks that have not been taken out yet, by `next` or
`wait`.

### Sender

#### Sender.send

```tondo
pub fn Sender.send(self, value: T)
```

Waits while the channel is full; sending to a closed channel panics.

#### Sender.close

```tondo
pub fn Sender.close(self)
```

Closes the channel: receivers get the values sent before, then `none`;
sending after it panics.

#### Sender.isClosed

```tondo
pub fn Sender.isClosed(self): Bool
```

Whether the channel is closed.

#### Sender.isAbandoned

```tondo
pub fn Sender.isAbandoned(self): Bool
```

Whether no copy of the channel's receiver is left, so what is sent can
never be received: a producer stops.

#### Sender.copy

```tondo
pub fn Sender.copy(self): Sender[T]
```

Another sender of the same channel.

### Receiver

#### Receiver.receive

```tondo
pub fn Receiver.receive(self, timeout: time.Duration? = none): T?
```

The next value, or `none` once closed and empty, or after `timeout`.

#### Receiver.isClosed

```tondo
pub fn Receiver.isClosed(self): Bool
```

Whether the channel is closed; values sent before may still wait to be
received.
