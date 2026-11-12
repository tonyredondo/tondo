# std.console

the program's standard output, error and input.

```tondo
import std.console

console.print("What is your name? ")
let name = console.readLine() ?? "stranger"
console.println("Hello, {name}!")
console.eprintln("greeted {name}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.console` shows the same in a terminal.

- [Functions](#functions): [`print`](#print), [`println`](#println), [`eprint`](#eprint), [`eprintln`](#eprintln), [`readLine`](#readline), [`flush`](#flush), [`stdout`](#stdout), [`stderr`](#stderr), [`stdin`](#stdin), [`setOutput`](#setoutput), [`setError`](#seterror), [`setInput`](#setinput)

### Functions

#### print

```tondo
pub fn print(text: String)
```

Writes to standard output, which the runtime buffers and delivers; it
never waits and cannot fail.

#### println

```tondo
pub fn println(text: String)
```

Writes the text and one LF to standard output.

#### eprint

```tondo
pub fn eprint(text: String)
```

Writes to standard error, unbuffered.

#### eprintln

```tondo
pub fn eprintln(text: String)
```

Writes the text and one LF to standard error.

#### readLine

```tondo
pub fn readLine(): String?
```

The next line of standard input without its `\n` or `\r\n`, or `none` at
the end.

#### flush

```tondo
pub fn flush(): !io.IoError
```

Delivers what standard output holds; the first failure since the last
flush.

#### stdout

```tondo
pub fn stdout(): io.Writer
```

The current streams, as `std.io` streams.

#### stderr

```tondo
pub fn stderr(): io.Writer
```

The current standard error, as a `std.io` stream.

#### stdin

```tondo
pub fn stdin(): io.Reader
```

The current standard input, as a `std.io` stream.

#### setOutput

```tondo
pub fn setOutput(writer: io.Writer)
```

Replaces a stream for the whole program, all its tasks included.

#### setError

```tondo
pub fn setError(writer: io.Writer)
```

Replaces standard error for the whole program, all its tasks included.

#### setInput

```tondo
pub fn setInput(reader: io.Reader)
```

Replaces standard input for the whole program, all its tasks included.
