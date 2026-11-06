# Runbook: finding inputs that fail

A function that takes outside input has inputs nobody thought of. A test that
calls `testing.fuzz` searches for them, and keeps the ones it finds.

```sh
tondo init
```

```tondo file=src/main.to
fn digit(text: String): Int {
    let chars: Array[Char] = text.chars().collect()
    if chars.length() > 2 and chars[0] == 'a' and chars[1] == 'b' {
        return 10 / (chars.length() - 3)
    }
    0
}

fn main() {}
```

```file src/main_test.to
import std.testing

test digits {
    testing.fuzz("digit", ["", "a", "abcd"], (text: String) {
        _ = digit(text)
    })?
}
```

`fuzz` is the seconds to search. The failure comes reduced to a small input,
with its panic, and stays in the project as a case every `test` runs again.

```mcp
test {"fuzz": 20}
=> P0003
```

```edit src/main.to
-        return 10 / (chars.length() - 3)
+        return 10 / chars.length()
```

```mcp
test {}
=> digits ... ok
```

`flags` takes `--fuzz-timeout` and `--fuzz-memory`, for inputs that hang or eat
memory. `debug_start` with `test` debugs the input that failed.
