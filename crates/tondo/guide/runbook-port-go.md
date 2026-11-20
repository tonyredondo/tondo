# Runbook: porting from Go

Go and Tondo agree on much: one executable, tasks that are cheap, errors as
values, no inheritance. So a port keeps the program's shape, and what changes
is what Go makes its author write by hand: the `if err != nil`, the
`WaitGroup`, the check for `nil`. `runbook-port` has the method every port
follows; this is what each thing of Go becomes.

## What each thing becomes

| Go | Tondo |
| --- | --- |
| `x := 5`, `var x int` | `let x = 5`, and `var x = 0` for what changes |
| `type Line struct { Name string }` | `type Line = { name: String }` |
| `func (l Line) Total() int` | `fn Line.total(self): Int` |
| `func (l *Line) Add(n int)`, which changes it | `fn Line.add(mut self, n: Int)` |
| a pointer parameter the function writes through | `fn bump(count: var Int)`, called `bump(var count)` |
| `func parse(s string) (Line, error)` | `fn parse(text: String): Line ! ParseError` |
| `return Line{}, err` and `return line, nil` | `fail error`, and the value as the last line |
| `v, err := f()` then `if err != nil { return err }` | `let v = f()?` |
| `fmt.Errorf("reading %s: %w", path, err)` | `f().context("reading {path}")?`, a `Failure` |
| `errors.Is`, `errors.As` | `match` on the error; `failure.find[E]()` |
| a pointer that may be `nil` | `T?`, with `none`, `match`, `?.` and `?? fallback` |
| `v, ok := m[k]` | `m.get(k)`, a `V?`; `m[k] ?? 0` |
| `[]T`, `append(s, x)`, `len(s)`, `s[1:3]` | `Array[T]`, `s.push(x)`, `s.length()`, `s[1:3]` |
| `map[K]V{}`, `delete(m, k)`, `m[k]++` | `Map[K, V]`, `[:]`, `m.remove(k)`, `m[k, default: 0] += 1` |
| `for i, x := range xs`, `for k, v := range m` | `for (i, x) in xs.enumerate()`, `for (k, v) in m` |
| `for cond { }`, `for { }` | the same |
| `switch` | `match`, which must cover every case |
| `const ( A = iota; B )` | `enum` |
| `interface` | `trait`, and `impl Trait for Type` says who implements it |
| `interface{}`, `any` | an `enum` of the cases, or a type parameter |
| an embedded struct | a field, and the methods that pass the call on |
| `go f()` with a `sync.WaitGroup` | `spawn f()`; `xs.map((x) { spawn f(x) }).wait()` waits for many and gives their results |
| `go f()` that outlives its function | `spawn detached f()` |
| `make(chan T, n)`, `<-ch`, `select` | `channel[T](n)`, `receive()`, `select` |
| a `sync.Mutex` beside what it guards | `sync.Mutex(value)`: the lock holds the value |
| a `context.Context` passed down | a `context`, set with `with`; cancelling a task reaches what it started |
| `defer`, `panic`, `recover()` | `defer`, `panic`, `recover { ... }` |
| `fmt.Sprintf("%s: %05d", a, b)` | `"{a}: {b:05}"` |
| `x_test.go`, `func TestX(t *testing.T)` | `x_test.to`, `test x { assert(...) }` |
| `init()`, a variable of the package | a `context` whose field is computed when first read |
| `name_linux.go`, build tags | `name_linux.to`, `name_macos.to` |

## The story: a till, ported

The original reads sales, a line each, prices them in goroutines, and prints
the bill. Its `package` and `import` lines are left out here.

```go
type Line struct {
	Name     string
	Quantity int
	Cents    int
}

func (l Line) Total() int { return l.Quantity * l.Cents }

// A line as it is written, tea,2,250, or why it is not one.
func parse(text string) (Line, error) {
	parts := strings.Split(text, ",")
	if len(parts) != 3 {
		return Line{}, fmt.Errorf("it has %d parts, not 3", len(parts))
	}
	quantity, err := strconv.Atoi(parts[1])
	if err != nil {
		return Line{}, fmt.Errorf("`%s` is no number", parts[1])
	}
	cents, err := strconv.Atoi(parts[2])
	if err != nil {
		return Line{}, fmt.Errorf("`%s` is no number", parts[2])
	}
	return Line{parts[0], quantity, cents}, nil
}

func money(cents int) string {
	return fmt.Sprintf("%d.%02d", cents/100, cents%100)
}

func main() {
	var lines []Line
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		line, err := parse(scanner.Text())
		if err != nil {
			fmt.Printf("skipped `%s`: %v\n", scanner.Text(), err)
			continue
		}
		lines = append(lines, line)
	}
	totals := make([]int, len(lines))
	var wg sync.WaitGroup
	for i, line := range lines {
		wg.Add(1)
		go func() {
			defer wg.Done()
			totals[i] = line.Total()
		}()
	}
	wg.Wait()
	sum := 0
	var dearest *Line
	for i, line := range lines {
		fmt.Printf("%s: %s\n", line.Name, money(totals[i]))
		sum += totals[i]
		if dearest == nil || totals[i] > dearest.Total() {
			dearest = &lines[i]
		}
	}
	fmt.Printf("total %s\n", money(sum))
	if dearest != nil {
		fmt.Printf("dearest: %s\n", dearest.Name)
	}
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | go run .
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with Go's forms

Written as Go writes, the port has a `struct`, a receiver, `go`, `:=` and
`range`. `check` answers each with Tondo's.

```tondo error E0004 file=orders.to
import std.console

type Line struct {
    name: String
    quantity: Int
    cents: Int
}

func (l Line) total() Int {
    return l.quantity * l.cents
}

fn work() {
}

fn main() {
    go work()
    lines := [1, 2]
    for _, line := range lines {
        console.println("{line}")
    }
}
```

```mcp
check {"path": "orders.to"}
=> a record is `type Line = { field: Type }`
=> a method is declared on its type, `fn Line.total(self): Result`
=> a task is `spawn work()`
=> a new binding is `let lines = value`
=> over pairs `for (index, x) in xs.enumerate()`
```

With the forms right, Go's names are the next round: its built-ins, `nil`,
and the packages of its library, for which `check` names what stands in
their place.

```tondo error E1001 file=orders.to
type Line = { name: String, quantity: Int, cents: Int }

fn parse(text: String): Line ! String {
    let parts = text.split(",")
    if len(parts) != 3 {
        fail "it has {len(parts)} parts, not 3"
    }
    let quantity = strconv.Atoi(parts[1])
    if quantity == nil {
        fail "`{parts[1]}` is no number"
    }
    Line { name: parts[0], quantity: 0, cents: 0 }
}

fn main() {}
```

```mcp
check {"path": "orders.to"}
=> unknown name `len`; use `value.length()`
=> unknown name `nil`; Tondo writes `none`
=> unknown name `strconv`; `Int.parse(text)` and `Float.parse(text)` read a number
```

`api` with a name gives how it is called and what it fails with; with
`search`, what does a job that no hint named.

```mcp
api {"module": "Int.parse"}
=> Int ! text.TextError
```

`Int.parse` gives a number or fails, and `??` takes what to do when it
fails, which may be to fail with the function's own error. That is the three
lines of `strconv.Atoi` and `if err != nil` in one.

## The port

```tondo file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn Line.total(self): Int {
    self.quantity * self.cents
}

/// A line as it is written, `tea,2,250`, or why it is not one.
fn parse(text: String): Line ! String {
    let parts = text.split(",")
    if parts.length() != 3 {
        fail "it has {parts.length()} parts, not 3"
    }
    let quantity = Int.parse(parts[1]) ?? fail "`{parts[1]}` is no number"
    let cents = Int.parse(parts[2]) ?? fail "`{parts[2]}` is no number"
    Line { name: parts[0], quantity, cents }
}

fn money(cents: Int): String {
    "{cents / 100}.{cents % 100:02}"
}

fn main() {
    var lines: Array[Line] = []
    for {
        let text = console.readLine() ?? break
        match parse(text) {
            ok(line) => lines.push(line)
            err(why) => console.println("skipped `{text}`: {why}")
        }
    }
    let totals = lines.map((line) { spawn line.total() }).wait()
    for (index, line) in lines.enumerate() {
        console.println("{line.name}: {money(totals[index])}")
    }
    console.println("total {money(totals.sum())}")
    match lines.maxBy((line) { line.total() }) {
        some(dearest) => console.println("dearest: {dearest.name}")
        none => {}
    }
}
```

The same input, and the same six lines:

```mcp
run {"path": "orders.to", "stdin": "tea,2,250\ncake,x,400\nwater,1,100\nbread\n"}
=> skipped `cake,x,400`: `x` is no number
=> skipped `bread`: it has 1 parts, not 3
=> tea: 5.00
=> water: 1.00
=> total 6.00
=> dearest: tea
```

What went, line by line of the original:

- **The error's two values.** `parse` gives a `Line` or fails; a caller cannot
  read the line of a parse that failed, and `match` makes it take both cases.
- **The `WaitGroup`, the slice to write into and the index.** `map` starts a
  task for each line, and `wait()` on the array waits for them all and gives
  their results in its order.
- **The pointer that may be `nil`.** `maxBy` gives a `Line?`, which has to be
  matched before its name is read.
- **The loop for the sum and the largest**, which `sum()` and `maxBy` say.

## What does not carry over

- **The zero value.** A record has every field given, or a default written in
  its type: there is no `Line{}`. A `Map` that was never made cannot be
  written to by mistake, because there is no `nil` map.
- **A goroutine left running.** A task nobody waits for is waited at the end
  of its block. One that must outlive its function says `detached`; a server
  does it for each connection.
- **A shared slice written from many goroutines.** Give each task what it
  needs and take what it returns, as above; what must be shared goes in a
  `sync.Mutex(value)` or a channel. `test` then tries the orders of the tasks
  by itself: `runbook-flaky`.
- **`interface{}` and reflection on any value.** Say which cases there are
  with an `enum`, or take a type parameter with a trait as its bound.
  `std.reflect` reads the fields of a type that is known.
- **Struct tags.** A field's annotations, as `@name("user_id")`, which hold
  in every format: `tondo guide stdlib`.
- **`go generate`, cgo.** `derive` and generators (`tondo guide meta`); C is
  called with `extern fn`, and `bind` writes the declarations (`tondo guide
  ffi`).

The original's tests become the port's first (`runbook-port`), its benchmarks
tests that call `testing.bench`, and `go test -fuzz` a test with
`testing.fuzz`: `runbook-testing`.
