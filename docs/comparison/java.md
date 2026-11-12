# Tondo for someone who writes Java

Modern Java has moved toward much of what Tondo starts from: records,
sealed types, a `switch` that matches patterns, threads that are cheap. A
Java programmer will recognize a good deal here. What Tondo does not carry
is what Java cannot drop: `null` in every reference, objects that are
shared by default, exceptions that half the methods declare and half do
not, and a virtual machine under the program. This page writes twelve
small programs in both. Every one was run, and prints what is shown.

- [The short version](#the-short-version)
- [Values and references](#values-and-references)
- [A value that may be missing](#a-value-that-may-be-missing)
- [Errors](#errors)
- [One of several things](#one-of-several-things)
- [Interfaces, traits and generics](#interfaces-traits-and-generics)
- [Collections and streams](#collections-and-streams)
- [Tasks](#tasks)
- [Deadlines and cancelling](#deadlines-and-cancelling)
- [Shared state](#shared-state)
- [JSON](#json)
- [Tests](#tests)
- [An HTTP server and its client](#an-http-server-and-its-client)
- [The toolchain, command by command](#the-toolchain-command-by-command)
- [Habits to leave behind](#habits-to-leave-behind)
- [Where Java is the better choice today](#where-java-is-the-better-choice-today)

## The short version

| | Java | Tondo |
| --- | --- | --- |
| A value that may be missing | `null`, in any reference; `Optional` by convention | `T?`, which has to be opened; there is no `null` |
| A function fails | throws: checked exceptions in the signature, unchecked ones not | returns `T ! E`, passed on with `?` |
| Assigning a list or an object | shares it | copies it; the storage is shared until one changes |
| Data | classes; records since Java 16 | records, enums and tuples: all values |
| One of several shapes | a sealed interface and records | an `enum` with data |
| Reuse | inheritance and interfaces | traits and composition; there is no inheritance |
| Generics | erased: a `List<Integer>` holds boxed objects | compiled for each type: an `Array[Int]` holds integers |
| Primitives and objects | two worlds, `int` and `Integer` | one: every type is a value |
| Threads | platform threads, and virtual ones since Java 21 | tasks, which their block waits for |
| A lock | `synchronized`, or a `Lock` beside the data | a `sync.Mutex[T]` around the data |
| A data race | possible | does not compile |
| An integer overflows | wraps | panics, in every build |
| JSON | a library | the standard library |
| What ships | a jar for a JVM, or a native image | one native executable |
| Starting | a virtual machine to warm up | at once |

## Values and references

Every Java object is reached through a reference, and assigning copies the
reference. A record cannot be changed, which avoids the surprise, at the
price of building each changed copy by hand:

```java
import java.util.ArrayList;
import java.util.List;

public class Values {
    record Point(int x, int y) {}

    public static void main(String[] args) {
        List<Integer> original = new ArrayList<>(List.of(1, 2, 3));
        List<Integer> alias = original; // the same list under another name
        alias.set(0, 99);
        alias.add(4);
        System.out.println(original + " " + alias);

        Point home = new Point(1, 2);
        Point away = new Point(10, home.y()); // a record cannot change: a changed copy is built by hand
        System.out.println(home.x() + " " + away.x());
    }
}
```

```text
[99, 2, 3, 4] [99, 2, 3, 4]
1 10
```

In Tondo a list is a value, as an `int` is in Java. Assigning it gives the
other name its own, and a record's copy can simply be changed:

```tondo
import std.console

type Point = { x: Int, y: Int }

fn main() {
    let original = [1, 2, 3]
    var copy = original                 // a copy: the two are apart from here on
    copy[0] = 99
    copy.push(4)
    console.println("{original} {copy}")

    let home = Point { x: 1, y: 2 }
    var away = home
    away.x = 10
    console.println("{home.x} {away.x}")
}
```

```text output
[1, 2, 3] [99, 2, 3, 4]
1 10
```

- No defensive copies, no `List.copyOf`, no `Collections.unmodifiableList`
  around a field before returning it. What a function was given is its own.
- The copy costs nothing until one of the two changes: they share their
  storage until then.
- `point with { x: 10 }` is a changed copy of a record, which Java has no
  syntax for.
- There is no boxing. An `Array[Int]` is integers side by side, a `Map[Int,
  Float]` holds them directly, and `==` compares values, strings included:
  there is no `equals` to remember.
- `let` is `final`, and it is the short one to write. `var` can change.

## A value that may be missing

Any Java reference can be `null`. `Optional` helps where it is used, and
it is a library type that nothing obliges a method to use:

```java
import java.util.List;
import java.util.Optional;

public class Missing {
    // `email` may be null. Nothing in the type says so.
    record User(String name, String email) {}

    static Optional<User> find(List<User> users, String name) {
        return users.stream().filter(user -> user.name().equals(name)).findFirst();
    }

    public static void main(String[] args) {
        var users = List.of(new User("ada", "ada@example.com"), new User("alan", null));
        for (var name : List.of("ada", "alan", "grace")) {
            // Optional is a library type: a method may still return null instead.
            var email = find(users, name).map(User::email).orElse("no email");
            System.out.println(name + ": " + email);
        }
    }
}
```

In Tondo there is no `null`. A `String` is always a string, and a field
that may be missing says so in its type:

```tondo
import std.console

type User = { name: String, email: String? }

fn find(users: Array[User], name: String): User? {
    users.find((user) { user.name == name })
}

fn main() {
    let users = [
        User { name: "ada", email: "ada@example.com" },
        User { name: "alan", email: none },
    ]
    for name in ["ada", "alan", "grace"] {
        // A user that may not be there, with an email that may not be there.
        let email = find(users, name).andThen((user) { user.email }) ?? "no email"
        console.println("{name}: {email}")
    }
}
```

```text output
ada: ada@example.com
alan: no email
grace: no email
```

- `email: String?` is in the record's type, where the Java record has a
  comment. A `NullPointerException` is not an error a Tondo program has.
- `andThen`, `map` and `unwrapOr` are `Optional`'s `flatMap`, `map` and
  `orElse`. `match` opens an option when both cases need code.
- `map.get(key)` gives an option, where `Map.get` gives `null` both for a
  missing key and for a key whose value is `null`.
- A field with no default has to be given when a record is made. No object
  exists half initialized.

## Errors

Java's checked exceptions are the nearest thing in a mainstream language to
what Tondo does: the signature says what can go wrong. They cover half of
it, since `NumberFormatException` and every other `RuntimeException` pass
through any signature unseen:

```java
import java.util.List;
import java.util.Map;

public class Errors {
    // A checked exception: methods that let it out must say `throws`.
    static class ConfigException extends Exception {
        ConfigException(String message) {
            super(message);
        }
    }

    static int port(Map<String, String> settings, String key) throws ConfigException {
        var text = settings.get(key);
        if (text == null) {
            throw new ConfigException(key + " is missing");
        }
        int value;
        try {
            value = Integer.parseInt(text);
        } catch (NumberFormatException error) { // unchecked: no signature mentions it
            throw new ConfigException(key + ": \"" + text + "\" is not a number");
        }
        if (value < 1 || value > 65535) {
            throw new ConfigException(key + ": " + value + " is out of range");
        }
        return value;
    }

    static String address(Map<String, String> settings) throws ConfigException {
        return "0.0.0.0:" + port(settings, "port");
    }

    public static void main(String[] args) {
        List<Map<String, String>> all =
                List.of(Map.of("port", "8080"), Map.of(), Map.of("port", "http"), Map.of("port", "70000"));
        for (var settings : all) {
            try {
                System.out.println(address(settings));
            } catch (ConfigException error) {
                System.out.println("error: " + error.getMessage());
            }
        }
    }
}
```

Tondo has one kind of failure for what a caller should handle, and it is
always in the signature:

```tondo
import std.console

enum ConfigError {
    Missing(String)
    NotANumber(String, String)
    OutOfRange(String, Int)
}

impl Display for ConfigError {
    fn display(self): String {
        match self {
            ConfigError.Missing(key) => "{key} is missing"
            ConfigError.NotANumber(key, text) => "{key}: \"{text}\" is not a number"
            ConfigError.OutOfRange(key, value) => "{key}: {value} is out of range"
        }
    }
}

fn port(settings: Map[String, String], key: String): Int ! ConfigError {
    let text = match settings.get(key) {
        some(found) => found
        none => fail ConfigError.Missing(key)
    }
    let value = Int.parse(text).mapErr((_) { ConfigError.NotANumber(key, text) })?
    if value < 1 or value > 65535 {
        fail ConfigError.OutOfRange(key, value)
    }
    value
}

/// The address to listen on: a failure of `port` passes through `?`.
fn address(settings: Map[String, String]): String ! ConfigError {
    "0.0.0.0:{port(settings, "port")?}"
}

fn main() {
    for settings in [["port": "8080"], [:], ["port": "http"], ["port": "70000"]] {
        match address(settings) {
            ok(found) => console.println(found)
            err(error) => console.println("error: {error}")
        }
    }
}
```

```text output
0.0.0.0:8080
error: port is missing
error: port: "http" is not a number
error: port: 70000 is out of range
```

- `Int ! ConfigError` is `throws ConfigException`, for every failure.
  `Int.parse` returns a result, so there is no unchecked exception to learn
  about from a stack trace.
- `?` passes a failure on in an expression, and it works inside a closure.
  A checked exception does not cross a lambda, which is why streams and
  checked exceptions do not mix.
- The error is a value: an `enum` whose cases the compiler knows. `match`
  on it must cover them all, and there is no hierarchy of exception classes
  to catch too much of.
- A panic is the other kind: a bug, as an index out of range. It is not
  declared and not caught in ordinary code, which is what unchecked
  exceptions were meant to be.
- `defer` is `finally`, and `try`-with-resources is a `defer` or a value
  that closes when its block ends.

## One of several things

Sealed interfaces and pattern matching brought this to Java, and it does it
well: the `switch` below needs no `default`, and fails to compile if a case
is missing:

```java
import java.util.List;
import java.util.Locale;

public class Data {
    sealed interface Shape permits Circle, Rectangle, Triangle {}

    record Circle(double radius) implements Shape {}

    record Rectangle(double width, double height) implements Shape {}

    record Triangle(double a, double b, double c) implements Shape {}

    static double area(Shape shape) {
        // The interface is sealed, so the switch knows these are all the cases.
        return switch (shape) {
            case Circle(double radius) -> Math.PI * radius * radius;
            case Rectangle(double width, double height) -> width * height;
            case Triangle(double a, double b, double c) -> {
                double s = (a + b + c) / 2;
                yield Math.sqrt(s * (s - a) * (s - b) * (s - c));
            }
        };
    }

    public static void main(String[] args) {
        for (Shape shape : List.of(new Circle(1), new Rectangle(2, 3.5), new Triangle(3, 4, 5))) {
            System.out.println(String.format(Locale.ROOT, "%.2f", area(shape)));
        }
    }
}
```

A Tondo `enum` is the sealed interface and its records in one declaration:

```tondo
import std.console
import std.math

enum Shape {
    Circle(Float)
    Rectangle(Float, Float)
    Triangle(Float, Float, Float)
}

fn area(shape: Shape): Float {
    match shape {
        Shape.Circle(radius) => math.Pi * radius * radius
        Shape.Rectangle(width, height) => width * height
        Shape.Triangle(a, b, c) => {
            let s = (a + b + c) / 2.0
            math.sqrt(s * (s - a) * (s - b) * (s - c)) ?? 0.0
        }
    }
}

fn main() {
    for shape in [Shape.Circle(1.0), Shape.Rectangle(2.0, 3.5), Shape.Triangle(3.0, 4.0, 5.0)] {
        console.println("{area(shape):.2}")
    }
}
```

```text output
3.14
7.00
6.00
```

- A variant is `Shape.Circle(1.0)`, with no `new`, and `match` takes it
  apart as the record patterns do.
- An `enum` with no data is a Java `enum`. One with data is the sealed
  hierarchy, in four lines.
- There are no classes. A record is `type Point = { x: Float, y: Float }`,
  with equality, defaults for its fields and `with`. Behaviour is functions
  and methods, written outside the type: `fn Point.length(self): Float`.
- `math.sqrt` returns a result, since the root of a negative number is not
  a number: the failure is in the type, where `Math.sqrt` gives a `NaN` to
  find later.

## Interfaces, traits and generics

```java
import java.util.List;
import java.util.Optional;

public class Traits {
    interface Describe {
        String describe();
    }

    record Book(String title, int pages) implements Describe {
        public String describe() {
            return title + ", " + pages + " pages";
        }
    }

    record Song(String title, int seconds) implements Describe {
        public String describe() {
            return String.format("%s, %d:%02d", title, seconds / 60, seconds % 60);
        }
    }

    // The call goes through the interface, found as the program runs.
    static String announce(Describe item) {
        return "Now: " + item.describe();
    }

    static <T extends Comparable<T>> Optional<T> largest(List<T> items) {
        if (items.isEmpty()) {
            return Optional.empty();
        }
        T best = items.get(0);
        for (T item : items) {
            if (item.compareTo(best) > 0) {
                best = item;
            }
        }
        return Optional.of(best);
    }

    public static void main(String[] args) {
        System.out.println(announce(new Book("Dune", 612)));
        System.out.println(announce(new Song("Hey Jude", 431)));
        System.out.println(largest(List.of(3, 9, 4)) + " " + largest(List.of("pear", "apple")) + " "
                + largest(List.<Integer>of()));
    }
}
```

```text
Now: Dune, 612 pages
Now: Hey Jude, 7:11
Optional[9] Optional[pear] Optional.empty
```

A Tondo trait is implemented with an `impl` outside the type, so a type can
be given a trait by code that did not write it. Generics are compiled for
each type:

```tondo
import std.console

trait Describe {
    fn describe(self): String
}

type Book = { title: String, pages: Int }
type Song = { title: String, seconds: Int }

impl Describe for Book {
    fn describe(self): String {
        "{self.title}, {self.pages} pages"
    }
}

impl Describe for Song {
    fn describe(self): String {
        "{self.title}, {self.seconds / 60}:{self.seconds % 60:02}"
    }
}

/// Any type that can describe itself; resolved when compiling.
fn announce[T: Describe](item: T): String {
    "Now: {item.describe()}"
}

/// The largest of some values of any type that has an order.
fn largest[T: Ordered](items: Array[T]): T? {
    var best = items.first()?
    for item in items {
        if item > best {
            best = item
        }
    }
    best
}

fn main() {
    console.println(announce(Book { title: "Dune", pages: 612 }))
    console.println(announce(Song { title: "Hey Jude", seconds: 431 }))
    console.println("{largest([3, 9, 4])} {largest(["pear", "apple"])} {largest[Int]([])}")
}
```

```text output
Now: Dune, 612 pages
Now: Hey Jude, 7:11
some(9) some(pear) none
```

- `announce[T: Describe]` is resolved when compiling. There is no interface
  dispatch, and no boxing of the value passed.
- `largest[T: Ordered]` works on integers as they are. Java's works on
  `Integer`, boxed, since generics cannot hold primitives.
- A trait of your own can be implemented for `Int` or `String`, or for a
  type of another module. An interface has to be named by the class that
  implements it.
- There is no inheritance, no `abstract`, no `protected` and no `static`:
  a function that needs no value is a function.
- There is no overloading. A function has one name and one signature, with
  default and named arguments where Java would overload.

## Collections and streams

Streams, with a `LinkedHashMap` to keep the words in order:

```java
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.stream.Collectors;

public class Collections {
    public static void main(String[] args) {
        var text = "to be or not to be that is the question to be";
        var words = text.split(" ");

        // A LinkedHashMap, to keep the order the words came in.
        Map<String, Long> counts = Arrays.stream(words)
                .collect(Collectors.groupingBy(word -> word, LinkedHashMap::new, Collectors.counting()));
        // The three most frequent; the sort is stable, so those that tie stay in order.
        var top = counts.entrySet().stream()
                .sorted(Map.Entry.<String, Long>comparingByValue().reversed())
                .limit(3)
                .map(entry -> entry.getKey() + " " + entry.getValue())
                .collect(Collectors.joining(", "));
        System.out.println(top);

        var lengths = Arrays.stream(words).mapToInt(String::length).toArray();
        var longWords = Arrays.stream(lengths).filter(n -> n > 2).count();
        System.out.println(longWords + " long words, " + Arrays.stream(lengths).sum() + " letters");
    }
}
```

Tondo's maps keep their order, and the operations of a stream are methods
of an array, with no `stream()` before and no `collect` after:

```tondo
import std.console

fn main() {
    let text = "to be or not to be that is the question to be"
    var counts: Map[String, Int] = [:]
    for word in text.split(" ") {
        counts[word] = (counts.get(word) ?? 0) + 1
    }
    // The three most frequent; those that tie stay in the order they came.
    let top = counts.entries().collect().sortedBy((entry) { -entry.1 }).take(3)
    console.println(top.map((entry) { "{entry.0} {entry.1}" }).join(", "))

    let lengths = text.split(" ").map((word) { word.length() })
    console.println("{lengths.filter((n) { n > 2 }).length()} long words, {lengths.sum()} letters")
}
```

```text output
to 3, be 3, or 1
4 long words, 34 letters
```

- `Array[T]`, `Map[K, V]`, `Set[T]` and `Deque[T]` are the collections.
  There is one of each, with no interface above it and no choice of
  implementation to make.
- A closure is `(word) { word.length() }`. It takes copies of the values
  around it, so nothing has to be "effectively final", and it cannot change
  a variable outside behind the reader's back: for that it is lent the
  variable, or shares an `Atomic`.
- `"{name} is {age}"` builds a string, with formats as `{price:.2}`. There
  is no `+` between strings and no `String.format`.
- A string's length is its characters, not UTF-16 units.

## Tasks

Virtual threads made Java's concurrency read the way Tondo's does: plain
code that blocks, cheaply. An executor in `try`-with-resources waits for
what it started:

```java
import java.util.List;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;

public class Tasks {
    record Job(String name, int delay) {}

    // Stands for a request that takes a while.
    static String fetch(String name, int delay) throws InterruptedException {
        Thread.sleep(delay);
        return name + " in " + delay + " ms";
    }

    public static void main(String[] args) throws Exception {
        long started = System.nanoTime();
        var jobs = List.of(new Job("users", 300), new Job("orders", 500), new Job("prices", 100));
        // A virtual thread for each job; closing the executor waits for them.
        try (var executor = Executors.newVirtualThreadPerTaskExecutor()) {
            List<Future<String>> futures =
                    jobs.stream().map(job -> executor.submit(() -> fetch(job.name(), job.delay()))).toList();
            for (var future : futures) {
                System.out.println(future.get());
            }
        }
        // One after another they would take 900 ms.
        System.out.println("all three in under 800 ms: " + ((System.nanoTime() - started) / 1_000_000 < 800));
    }
}
```

In Tondo that is how every task works, with no executor: a task belongs to
the block that started it, which waits for it.

```tondo
import std.console
import std.time

/// Stands for a request that takes a while.
fn fetch(name: String, delay: Int): String {
    time.sleep(time.milliseconds(delay))
    "{name} in {delay} ms"
}

fn main() {
    let started = time.now()
    var group = Group[String]()
    group.add(spawn fetch("users", 300))
    group.add(spawn fetch("orders", 500))
    group.add(spawn fetch("prices", 100))
    for result in group.wait() {            // in the order they were added
        console.println(result)
    }
    // One after another they would take 900 ms.
    console.println("all three in under 800 ms: {started.elapsed().milliseconds() < 800}")
}
```

```text output
users in 300 ms
orders in 500 ms
prices in 100 ms
all three in under 800 ms: true
```

- `spawn f()` gives a `Join[T]`, and `wait()` its result. An error in the
  task is its result's error; there is no `ExecutionException` wrapped
  around it, and no `InterruptedException` to declare on every method that
  sleeps.
- A `Group` waits for many and gives their results in order.
- A task that should outlive its block is `spawn detached f()`.
- Channels and `select` are in the language, for tasks that talk to each
  other.

## Deadlines and cancelling

A `Future` can be waited for with a limit. Stopping the work is a separate
step, and it stops only if the work checks for interruption:

```java
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public class Deadline {
    static String fetch(int delay) throws InterruptedException {
        Thread.sleep(delay);
        return "answered in " + delay + " ms";
    }

    public static void main(String[] args) throws Exception {
        try (var executor = Executors.newVirtualThreadPerTaskExecutor()) {
            for (int delay : new int[] {10, 5000}) {
                var future = executor.submit(() -> fetch(delay));
                try {
                    System.out.println(future.get(250, TimeUnit.MILLISECONDS));
                } catch (TimeoutException error) {
                    // Giving up on the answer does not stop the work: that is another call.
                    future.cancel(true);
                    System.out.println("gave up after 250 ms");
                }
            }
        }
    }
}
```

In Tondo the deadline goes around the block, and the work under it is
stopped when the time passes:

```tondo
import std.console
import std.time

fn fetch(delay: Int): String {
    time.sleep(time.milliseconds(delay))
    "answered in {delay} ms"
}

fn main() {
    for delay in [10, 5000] {
        // The deadline holds for all the block calls, and for the tasks it starts.
        match within time.milliseconds(250) { fetch(delay) } {
            ok(answer) => console.println(answer)
            err(_) => console.println("gave up after 250 ms")
        }
    }
}
```

```text output
answered in 10 ms
gave up after 250 ms
```

- Every wait can be cancelled. Nothing is caught and nothing is restored,
  as the interrupted flag has to be.
- The deadline holds for everything the block calls and every task it
  starts. An HTTP request or a query made inside has that long.
- What a `ThreadLocal` or a `ScopedValue` carries is a `context`: declared
  once, typed, read by name in any function below, and inherited by tasks.

## Shared state

```java
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Executors;

public class Shared {
    // The lock and what it guards are tied only by this comment:
    // every use of `hits` must be inside `synchronized (hits)`.
    static final Map<String, Integer> hits = new HashMap<>();

    static void work(String page) {
        for (int i = 0; i < 100; i++) {
            synchronized (hits) {
                hits.merge(page, 1, Integer::sum);
            }
        }
    }

    public static void main(String[] args) {
        try (var executor = Executors.newVirtualThreadPerTaskExecutor()) {
            for (int n = 0; n < 50; n++) {
                var page = n % 2 == 0 ? "/home" : "/about";
                executor.submit(() -> work(page));
            }
        }
        System.out.println(hits.get("/home") + " " + hits.get("/about"));
    }
}
```

A Tondo mutex holds the value it guards, so the comment above is a type:

```tondo
import std.console
import std.sync

fn work(hits: sync.Mutex[Map[String, Int]], page: String) {
    for _ in 0..100 {
        hits.update((counts) { counts[page] = (counts.get(page) ?? 0) + 1 })
    }
}

fn main() {
    let empty: Map[String, Int] = [:]
    let hits = sync.Mutex(empty)
    var group = Group[Unit]()
    for n in 0..50 {
        group.add(spawn work(hits, if n % 2 == 0 { "/home" } else { "/about" }))
    }
    _ = group.wait()
    let counts = hits.get()
    console.println("{counts.get("/home") ?? 0} {counts.get("/about") ?? 0}")
}
```

```text output
2500 2500
```

- The map cannot be reached without the lock. `update` has it while its
  function runs.
- A task cannot be given a plain variable to change: that does not
  compile. Java finds such a race in production, or with luck in a test.
- `std.sync` has `Atomic`, `RwLock`, `Semaphore`, `Once` and shared
  collections, the counterparts of `java.util.concurrent`.

## JSON

The JDK reads no JSON. With Jackson, as most programs:

```java
import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.util.List;

public class Json {
    record Person(String name, int age, List<String> languages, String email) {}

    public static void main(String[] args) throws JsonProcessingException {
        var mapper = new ObjectMapper();
        var text = """
                {"name": "Ada", "age": 36, "languages": ["en", "fr"]}""";
        var person = mapper.readValue(text, Person.class);
        System.out.println(person.name() + ", " + person.age() + ", " + person.languages() + ", " + person.email());
        var older = new Person(person.name(), 37, person.languages(), person.email());
        System.out.println(mapper.writeValueAsString(older));

        try {
            mapper.readValue("""
                    {"name": "Alan", "age": "forty"}""", Person.class);
        } catch (JsonProcessingException error) {
            System.out.println(error.getOriginalMessage());
        }
    }
}
```

```text
Ada, 36, [en, fr], null
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
Cannot deserialize value of type `int` from String "forty": not a valid `int` value
```

In Tondo it is the standard library, with code the compiler writes for
each type. No reflection runs:

```tondo
import std.console
import std.json

type Person = {
    name: String
    age: Int
    languages: Array[String] = []
    email: String?
}

fn main() {
    let text = r"""{"name": "Ada", "age": 36, "languages": ["en", "fr"]}"""
    match json.decode[Person](text) {
        ok(person) => {
            console.println("{person.name}, {person.age}, {person.languages}, {person.email}")
            console.println(json.encode(person with { age: 37 }))
        }
        err(error) => console.println("{error}")
    }
    // The wrong type in a field is an error that says where.
    match json.decode[Person](r"""{"name": "Alan", "age": "forty"}""") {
        ok(person) => console.println(person.name)
        err(error) => console.println("{error}")
    }
}
```

```text output
Ada, 36, [en, fr], none
{"name":"Ada","age":37,"languages":["en","fr"],"email":null}
expected an integer, found a string at $.age (line 1, column 32)
```

- A field that is not optional and has no default must be in the JSON.
  Jackson leaves a missing `email` `null` and a missing `int` zero.
- `@name("user_id")` on a field names it on the wire, and
  `@validate.Min(0)` is checked by the decoder, as Bean Validation is by a
  framework above.
- The same record reads and writes YAML, TOML, CBOR and MessagePack.

## Tests

With JUnit:

```java
import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class TitlesTest {
    @Test
    void capitalizesEachWord() {
        assertEquals("The Old Man And The Sea", Titles.titleCase("the old man and the sea"));
    }

    @Test
    void keepsOneSpaceBetweenWords() {
        assertEquals("War And Peace", Titles.titleCase("  war   AND peace "));
        assertEquals("", Titles.titleCase(""));
    }
}
```

In Tondo a test is a `test` block, in a file beside the code it tests:

```tondo test
/// The words of a title, each with a capital.
fn titleCase(text: String): String {
    text.split(" ").filter((word) { not word.isEmpty() }).map((word) { "{word[0:1].toUpper()}{word[1:].toLower()}" }).join(" ")
}

test capitalizesEachWord {
    assert(titleCase("the old man and the sea") == "The Old Man And The Sea")
}

test keepsOneSpaceBetweenWords {
    assert(titleCase("  war   AND peace ") == "War And Peace")
    assert(titleCase("") == "")
}
```

- No framework, no annotations, no test runner to configure in a build
  file. `tondo test` runs them.
- `assert(a == b)` prints both sides when it fails, so there is no
  `assertEquals(expected, actual)` to get the wrong way round.
- Benchmarks, fuzz tests, snapshots and coverage are in the same command,
  where Java has JMH, Jazzer and JaCoCo.

## An HTTP server and its client

The JDK has both, at a low level. Most programs use a framework:

```java
import com.fasterxml.jackson.databind.ObjectMapper;
import com.sun.net.httpserver.HttpServer;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

public class Http {
    record Greeting(String message, int length) {}

    public static void main(String[] args) throws Exception {
        var mapper = new ObjectMapper();
        var server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0); // a free port
        server.createContext("/hello/", exchange -> {
            var name = exchange.getRequestURI().getPath().substring("/hello/".length());
            var body = mapper.writeValueAsBytes(new Greeting("Hello, " + name + "!", name.length()));
            exchange.getResponseHeaders().add("Content-Type", "application/json");
            exchange.sendResponseHeaders(200, body.length);
            try (var out = exchange.getResponseBody()) {
                out.write(body);
            }
        });
        server.start();
        var url = "http://127.0.0.1:" + server.getAddress().getPort();

        var client = HttpClient.newHttpClient();
        var response = client.send(
                HttpRequest.newBuilder(URI.create(url + "/hello/Ada")).build(), HttpResponse.BodyHandlers.ofString());
        var greeting = mapper.readValue(response.body(), Greeting.class);
        System.out.println(greeting.message() + " (" + greeting.length() + ")");

        var missing = client.send(
                HttpRequest.newBuilder(URI.create(url + "/nothing")).build(), HttpResponse.BodyHandlers.ofString());
        System.out.println(missing.statusCode()); // a 404 is not an exception here: the code has to look
        server.stop(0);
    }
}
```

The same in Tondo:

```tondo
import std.console
import std.http

type Greeting = { message: String, length: Int }

fn main(): !http.HttpError {
    var app = http.App()
    app.get("/hello/:name", (request) {
        let name = request.param("name")
        http.Reply.json(Greeting { message: "Hello, {name}!", length: name.length() })
    })
    let server = app.start()?                   // on a free port
    defer server.stop()

    let greeting: Greeting = http.get("{server.url()}/hello/Ada")?.json()?
    console.println("{greeting.message} ({greeting.length})")
    match http.get("{server.url()}/nothing") {
        err(http.HttpError.Status(response)) => console.println("{response.status()}")
        _ => console.println("unexpected")
    }
}
```

```text output
Hello, Ada! (3)
404
```

- A route has its parameters, `"/hello/:name"`, and a typed handler takes
  them as arguments: `app.get("/users/:id", (id: Int) { find(id) })`. What
  it returns is written as JSON.
- There is no container, no annotations scanned at start and no
  dependency injection: a handler is a function, and what it needs is a
  value it captures or a `context` it reads.
- The client fails with an error for a status of 400 or more.
- The program starts in milliseconds, which matters for a command line and
  for a service that scales from nothing.

## The toolchain, command by command

| Java | Tondo | |
| --- | --- | --- |
| `java Main.java`, `mvn exec:java` | `tondo run` | compiles, keeps the build, runs |
| `mvn package`, `gradle build`, `jlink`, `native-image` | `tondo build` | one native executable; `--static`, `--target aarch64-linux` |
| `pom.xml`, `build.gradle` | `tondo.toml` | a few lines; no build to program |
| `mvn test`, JUnit | `tondo test` | with `--bench`, `--fuzz`, `--coverage` |
| google-java-format, Checkstyle, Error Prone | `tondo fmt`, `tondo check`, `tondo fix` | one layout, no configuration |
| Maven Central | `tondo add` | dependencies by git or URL, locked in `tondo.lock` |
| `jdb`, the IDE's debugger | `tondo debug` | also backwards, over a recording |
| JFR, `jcmd`, `jstack` | `tondo profile`, `tondo counters`, `tondo dump` | on a program started with `TONDO_DIAGNOSTICS=1` |
| JMH | `tondo test --bench` | with baselines and a limit on regressions |
| JNI, the foreign function API | `extern fn`, `tondo bind` | declarations written from a C header |
| annotation processors | `derive` and generators | part of the build |
| a JVM to install and tune | nothing | the runtime is in the executable |

## Habits to leave behind

| In Java | In Tondo |
| --- | --- |
| `int x = 1;`, `final var y = 2;` | `var x = 1`, `let y = 2` |
| `String name = null;` | `let name: String? = none` |
| `Optional<User>` | `User?` |
| `a.equals(b)` | `a == b` |
| `throw new IllegalStateException(...)` | `fail error`, in a function that returns `T ! E` |
| `try { } catch (E e) { }` | `match` on the result |
| `throws IOException` | `! fs.FsError` |
| `class`, `extends`, `implements` | `type`, `enum`, `trait`, `impl` |
| `new Point(1, 2)` | `Point { x: 1, y: 2 }` |
| `point.x()` | `point.x` |
| `public`, `private`, `protected` | `pub`, or nothing |
| `static` methods in a utility class | functions of a module |
| `List<T>`, `Map<K, V>`, `Set<T>` | `Array[T]`, `Map[K, V]`, `Set[T]` |
| `list.stream().map(f).toList()` | `list.map(f)` |
| `x -> x * 2` | `(x) { x * 2 }` |
| `String.format("%s is %d", name, age)` | `"{name} is {age}"` |
| `"a" + b` | `"a{b}"` |
| `for (var x : xs)`, `while (c)` | `for x in xs`, `for c` |
| `switch` | `match` |
| `&&`, `\|\|`, `!` | `and`, `or`, `not` |
| `(int) x`, `instanceof` | `Int(x)`, which fails if it does not fit; `match` |
| `synchronized (lock) { }` | `mutex.update((value) { ... })` |
| `executor.submit(...)`, `future.get()` | `spawn f()`, `job.wait()` |
| `@Annotation` | `@annotation`, which is only data |
| `package` and `import` | a directory is a module; `import app.orders` |
| `;` | nothing |

`tondo guide pitfalls` has these and the error each gives.

## Where Java is the better choice today

- **Its ecosystem and its age.** Thirty years of libraries, frameworks,
  monitoring and people. Whatever a company runs has a Java client. Tondo
  is in early development and can still change.
- **Long-running throughput.** A JIT that has watched a program for an hour
  optimizes it beyond what an ahead-of-time compiler does, and the JVM's
  collectors are the product of decades.
- **Android**, and the other languages of the JVM that share its libraries.
- **Platforms.** A jar runs wherever there is a JVM. Tondo builds for Linux
  and macOS.
- **Loading code as the program runs**: plugins, agents, application
  servers.

What Tondo offers against that is in the programs above: no `null`, every
failure in a signature, values that are not shared by accident, no boxes,
and an executable of a few megabytes that starts at once and needs nothing
installed beside it.

Next: [Tondo for someone who writes Kotlin](kotlin.md).
