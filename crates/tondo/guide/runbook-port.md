# Runbook: porting code to Tondo

Code that works in another language is to work the same in Tondo. Port what it
does, not its lines: the lines carry the habits of their language, and Tondo
often asks for less. The order below keeps every step small and checked. The
runbooks of each language say what each thing becomes: `runbook-port-go`,
`runbook-port-csharp`, `runbook-port-rust`, `runbook-port-python`,
`runbook-port-typescript`, `runbook-port-java`, `runbook-port-cpp`,
`runbook-port-swift` and `runbook-port-kotlin`.

1. Read `tondo guide pitfalls` and the runbook of the language once.
2. Find what says what the code does: its tests, or its output for inputs you
   choose. That is the port's specification, and it is ported first.
3. Go from the leaves: the code that depends on nothing of its own, then what
   uses it. A package or namespace becomes a directory, which is a module.
4. Port a function at a time, with `check` after each. For every call to the
   original's library, `api` with `search`: a name guessed from another
   language is the commonest mistake.
5. `test`, then the same input to both programs and the same output.
6. Only then make it Tondo's: what the language's runbook says changes shape.
7. Measure, when speed is why the code is ported: `runbook-performance`.

## The story: the original's cases come first

This is the original, in Python, with its tests: a function, and the cases
that say what it does.

```python
def initials(name):
    """The first letter of each word, in upper case: ada lovelace -> AL."""
    return "".join(word[0].upper() for word in name.split())


def test_initials():
    assert initials("ada lovelace") == "AL"
    assert initials("  grace   hopper ") == "GH"
    assert initials("") == ""
```

```sh
tondo init
```

The port starts with the signature, a body that says it is not there yet, and
the cases, which are the same three. `check` reads the tests too, so they
compile before the function is written.

```tondo file=src/main.to
/// The first letter of each word, in upper case: `ada lovelace` gives `AL`.
fn initials(name: String): String {
    panic("`initials` is not ported yet: {name}")
}

fn main() {}
```

```file src/main_test.to
test initialsOfEachWord {
    for (name, expected) in [("ada lovelace", "AL"), ("  grace   hopper ", "GH"), ("", "")] {
        assert(initials(name) == expected, "{name}")
    }
}
```

```mcp
check {}
=> it compiles
test {}
=> initialsOfEachWord ... FAILED
=> not ported yet
```

Now the body. Python's `split()` with no argument splits on runs of
whitespace, and nothing says that Tondo's `split` does: look it up before
writing it. What does that job has another name here.

```mcp
api {"search": "words"}
=> String.words
api {"module": "String.words"}
=> The pieces between runs of whitespace
```

```edit src/main.to
-    panic("`initials` is not ported yet: {name}")
+    name.words().map(word => word.slice(0, 1) ?? "").join("").toUpper()
```

```mcp
check {}
=> it compiles
test {}
=> initialsOfEachWord ... ok
```

The cases pass, the middle one too, which a `split(" ")` would have failed.
`coverage` then says which lines of the port no case of the original runs:
each is a behaviour nobody wrote down, to test before trusting it.

```mcp
test {"coverage": true}
=> lines 100.0%
```

## What to carry over, and what to leave

- **The behaviour, with its edges**: the empty input, the error's words when
  a program's users read them, the order of what it prints.
- **The names**, in Tondo's form: `camelCase` for functions and bindings,
  `PascalCase` for types, and a module for each package.
- **Not the workarounds**: a lock around what a task now owns, a check for a
  null that a type now rules out, a loop that `map` and `filter` say in a
  line. `check` warns of some, and the language's runbook names the rest.
- **Not the dependencies, at first**: the standard library has HTTP, JSON,
  SQL, templates, a command line and more. `api` with `list` names the
  modules, and `tondo guide stdlib` says what each is for. For what it has
  not, `add_dependency` adds a package from a directory, a git repository or
  an archive, `update` fetches them again and `remove_dependency` takes one
  out.
- **The examples of its documents**: `doc_test` checks the Tondo examples of
  a Markdown file, as the port's README.
- **C stays C**: a C library is called as it is, with `bind` and `extern fn`
  (`tondo guide ffi`), not ported.

When the port is done: `unreached` gives what was ported and nothing uses,
and `runbook-testing` the tests that the original never had.
