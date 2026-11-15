# Runbook: porting from C++

C++ makes its author decide who owns each value, how long it lives and what
happens when an index is wrong. Tondo decides the first two and stops at the
third. A port keeps the data and the algorithms and drops the rest: the
references and pointers, the destructors, the headers. A library with a C
interface is not ported: it is called as it is (`tondo guide ffi`).
`runbook-port` has the method every port follows; this is what each thing of
C++ becomes.

## What each thing becomes

| C++ | Tondo |
| --- | --- |
| `int x = 5;`, `auto x = 5;`, `const int n = 5;` | `var x = 5`, `let n = 5`, and no `;` |
| `int`, `long`, `size_t`, `uint8_t`, `double`, `bool`, `char` | `Int`, `Int`, `Int`, `Byte`, `Float`, `Bool`, `Char` |
| `struct Line { std::string name; };`, a class of fields | `type Line = { name: String }` |
| a member function, `const` or not | `fn Line.total(self): Int`; `mut self` to change a field |
| `const T&`, `T&&`, `std::move` | the value: there is nothing to write |
| a `T&` or `T*` the function writes through | `fn bump(count: var Int)`, called `bump(var count)` |
| a `T*` that may be null, `std::optional<T>` | `T?`, with `none` |
| `new` and `delete`, `unique_ptr`, `shared_ptr` | a value, let go when nothing holds it |
| a destructor that closes or unlocks | `defer` where the value is made; a lock lets go by itself |
| `std::vector<T>`, `std::unordered_map<K, V>`, `std::set<T>` | `Array[T]`, `Map[K, V]`, `Set[T]` |
| `v.push_back(x)`, `v.size()`, `v[i]`, `v.empty()` | `v.push(x)`, `v.length()`, `v[i]`, `v.isEmpty()` |
| `std::string`, `std::string_view`, `const char*` | `String` |
| `std::variant<A, B>`, a `union` with a tag | an `enum` whose variants hold data, and `match` |
| `throw`, `try` and `catch`, an error code, `std::expected<T, E>` | `T ! E`, `fail`, `?`, `match` |
| a base class with `virtual` functions | a `trait` and `impl`; an `enum` when the cases are known |
| `template <typename T>`, a concept | `fn largest[T: Ordered](xs: Array[T])` |
| `[&](int x) { return x * 2; }` | `(x) { x * 2 }`: it captures what it uses |
| `for (const auto& x : xs)`, `for (int i = 0; i < n; i++)` | `for x in xs`, `for i in 0..n` |
| `std::sort`, `std::accumulate`, `std::max_element`, `std::transform`, `std::find_if` | `sorted()`, `sum()`, `maxBy`, `map`, `find` |
| `std::format("{}: {:02}", a, b)`, `std::cout << x` | `"{a}: {b:02}"`, `console.println(...)` |
| `static_cast<double>(x)`, `std::stoi(s)`, `std::from_chars` | `Float(x)`, `Int.parse(s)` |
| `std::thread`, `std::async`, `std::future` | `spawn f()`, a `Group` |
| a `std::mutex` beside what it guards, `std::atomic<int>` | `sync.Mutex(value)`, `sync.Atomic(0)` |
| `#include`, a header and its source, `namespace shop` | `import`, one file, the directory `shop` |
| `#ifdef __linux__` | a file `name_linux.to` |
| a macro, `constexpr` | a function, `const`; `derive` for code from a type |
| `TEST(Orders, Parses) { EXPECT_EQ(a, b); }` | `test parses { assert(a == b) }`, in `orders_test.to` |

## The story: a till, ported

The original reads sales, a line each, prices them with `std::async`, and
prints the bill. Its `#include` lines are left out here.

```cpp
struct Line {
    std::string name;
    int quantity;
    int cents;

    int total() const { return quantity * cents; }
};

int number(const std::string& part) {
    int value = 0;
    const char* end = part.data() + part.size();
    auto [last, error] = std::from_chars(part.data(), end, value);
    if (error != std::errc{} || last != end) {
        throw std::invalid_argument(std::format("`{}` is no number", part));
    }
    return value;
}

// A line as it is written, tea,2,250; throws when it is not one.
Line parse(const std::string& text) {
    std::vector<std::string> parts;
    std::stringstream stream(text);
    for (std::string part; std::getline(stream, part, ',');) {
        parts.push_back(part);
    }
    if (parts.size() != 3) {
        throw std::invalid_argument(std::format("it has {} parts, not 3", parts.size()));
    }
    return Line{parts[0], number(parts[1]), number(parts[2])};
}

std::string money(int cents) {
    return std::format("{}.{:02}", cents / 100, cents % 100);
}

int main() {
    std::vector<Line> lines;
    for (std::string text; std::getline(std::cin, text);) {
        try {
            lines.push_back(parse(text));
        } catch (const std::invalid_argument& why) {
            std::cout << std::format("skipped `{}`: {}\n", text, why.what());
        }
    }
    std::vector<std::future<int>> pricing;
    for (const Line& line : lines) {
        pricing.push_back(std::async(std::launch::async, [&line] { return line.total(); }));
    }
    std::vector<int> totals;
    for (auto& job : pricing) {
        totals.push_back(job.get());
    }
    for (std::size_t i = 0; i < lines.size(); i++) {
        std::cout << std::format("{}: {}\n", lines[i].name, money(totals[i]));
    }
    std::cout << std::format("total {}\n", money(std::accumulate(totals.begin(), totals.end(), 0)));
    auto dearest = std::max_element(lines.begin(), lines.end(),
        [](const Line& a, const Line& b) { return a.total() < b.total(); });
    if (dearest != lines.end()) {
        std::cout << std::format("dearest: {}\n", dearest->name);
    }
}
```

What it prints for four lines, one with a letter where a number goes and one
too short, is what the port must print:

```text
$ printf 'tea,2,250\ncake,x,400\nwater,1,100\nbread\n' | ./orders
skipped `cake,x,400`: `x` is no number
skipped `bread`: it has 1 parts, not 3
tea: 5.00
water: 1.00
total 6.00
dearest: tea
```

## The first try, with C++'s habits

Written as C++ writes, the port has a `struct` with its `;`, a function with
its result first, a `const` reference, a `try`, `auto`, `++` and `std::cout`.
`check` answers each with Tondo's.

```tondo error E0004 file=orders.to
import std.console

struct Line {
    name: String
    quantity: Int
    cents: Int
};

int total(Line line) {
    return line.quantity * line.cents
}

fn money(cents: const Int&): String {
    "{cents / 100}.{cents % 100:02}"
}

fn number(part: String): Int {
    try {
        return Int.parse(part)
    } catch (const std::invalid_argument& why) {
        return 0
    }
}

fn main() {
    auto count = 0
    count++
    std::cout << count
}
```

```mcp
check {"path": "orders.to"}
=> a record is `type Line = { field: Type }`
=> `;` is not a Tondo token
=> a function is `fn total(value: Type): Int { ... }`, its result after its parameters
=> a parameter's type is the value's, as `line: Line`: there are no `const` types or references
=> there is no `try`: a function that can fail returns `T ! E`
=> a binding is `let count = value`, or `var count = value` for one that changes
=> `++` is not a Tondo operator: write `value += 1`
=> `console.println("total {x}")` writes a line
```

With the forms right, the names of the standard containers and the null
pointer are the next round: `size()`, `push_back` and `nullptr`.

```tondo error E1001 file=orders.to
import std.console

type Line = { name: String, quantity: Int, cents: Int }

fn dearest(lines: Array[Line]): Line? {
    if lines.size() == 0 {
        return nullptr
    }
    lines[0]
}

fn main() {
    var lines: Array[Line] = []
    lines.push_back(Line { name: "tea", quantity: 2, cents: 250 })
    console.println("{dearest(lines)}")
}
```

```mcp
check {"path": "orders.to"}
=> has no method `size`: use `length()`
=> unknown name `nullptr`; Tondo writes `none`
=> has no method `push_back`: use `push(value)`
```

What the standard library does, ask for by what it does.

```mcp
api {"search": "parse integer"}
=> Int.parse
```

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
    var pricing = Group[Int]()
    for line in lines {
        pricing.add(spawn line.total())
    }
    let totals = pricing.wait()
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

What went, part by part of the original:

- **Every `const`, `&` and `std::`.** A function takes the value and gives
  one back, with no copy made until one side changes it.
- **`from_chars`, its two results and the check of both.** `Int.parse` gives
  the number or fails.
- **The exception.** `parse` says in its signature that it can fail, and
  `match` takes both cases.
- **The futures and the reference each lambda captured.** `spawn` starts a
  task with the line it needs, and the `Group` gives their results in order.
  No task can outlive what it borrowed, because it borrows nothing.
- **The iterator compared with `end()`.** `maxBy` gives a `Line?`.

## What does not carry over

- **Undefined behaviour.** An index out of bounds, an overflow, a division by
  zero and a missing value each stop the program with a panic that says
  where; a value used after it is gone cannot be written at all. A port often
  finds the original's bugs this way: `runbook-crash`.
- **Deciding where a value lives.** There is no stack or heap to choose, no
  `new`, no smart pointer. The compiler keeps off the heap what it can prove
  stays in its function, and says what it decided: `runbook-allocations`.
- **Destructors as a way to run code.** What must happen when a block ends
  is a `defer`, written where the resource is taken.
- **Inheritance, and templates as a language of their own.** A contract is a
  trait, a closed family an `enum`; a type parameter takes a bound, and code
  written from a type is `derive` (`tondo guide meta`).
- **Arithmetic that wraps.** An `Int` has 64 bits and panics when a result
  does not fit, also the unsigned ones. Code that counts on wrapping, as a
  hash, says `wrappingAdd` and `wrappingMul`.
- **The preprocessor and the build.** No headers, no macros, no build files:
  `tondo build` makes the executable, and a file for one system is named for
  it, as `poll_linux.to`.

The original's tests become the port's first (`runbook-port`), its Google
Benchmark cases tests that call `testing.bench`, and a libFuzzer target a
test with `testing.fuzz`: `runbook-testing`.
