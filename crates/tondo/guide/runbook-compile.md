# Runbook: a program that does not compile

Each diagnostic says what is wrong and how to repair it, often with the code to
write. Work from the first: later ones are often its consequences. Do not guess
a name from another language: `api` says what exists.

```tondo error E1001 file=greet.to
import std.console

fn main() {
    let name = "Ada"
    console.println(name.upper())
}
```

```mcp
check {"path": "greet.to"}
=> has no method `upper`: use `toUpper()`
```

The diagnostic often says it all. When it does not, `api` with `search` finds
what does a job, and with a module or a type what it has, with an example that
compiles.

```mcp
api {"search": "upper case"}
=> toUpper
```

```edit greet.to
-    console.println(name.upper())
+    console.println(name.toUpper())
```

`check` again after every edit: it is the fastest loop there is. A warning does
not stop a build, but it often is a bug.

```mcp
check {"path": "greet.to"}
=> it compiles
```

Repairs the compiler is sure of are `fix`'s: it applies them all, and says what
it did.

```edit greet.to
-import std.console
+import std.console
+import std.time
```

```mcp
check {"path": "greet.to"}
=> W1001
fix {"path": "greet.to", "apply": true}
check {"path": "greet.to"}
=> it compiles
```

`explain` says more of a code, and `tondo guide pitfalls` what is often wrong
coming from other languages.
