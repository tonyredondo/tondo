# Runbook: a program of the terminal, where there is none

A program asks questions, draws a screen or reads keys, and what runs it has
no terminal: an agent's tools, a test, a job of CI. Everything of `std.cli`
works there by design: a question reads a line, what draws is text, and only
taking the terminal needs one. The parts are tested apart, and a run in a real
terminal is recorded, to open here.

## A program that asks

From a pipe each question reads a line: `choose` a number, `chooseMany`
several as `1,3`, `confirm` a `y`, and an empty line takes the default. So
`run` answers them with `stdin`, as a script or a test would. When an answer
depends on what the program said before, `run_start` keeps it running and
`run_input` writes one answer and gives what the program printed then.

```tondo file=order.to
import std.cli
import std.console

fn main() {
    let drinks = ["tea", "coffee", "water"]
    let drink = drinks[cli.choose("Which drink?", drinks)]
    let extras = cli.chooseMany("With what?", ["milk", "sugar", "lemon"])
    let name = cli.ask("For whom?", default: "the house")
    if cli.confirm("Send it?") {
        console.println("a {drink} with {extras.length()} extras for {name}")
    } else {
        console.println("nothing sent")
    }
}
```

```mcp
run {"path": "order.to", "stdin": "2\n1,3\nAda\ny\n"}
=> a coffee with 2 extras for Ada
```

## A program that takes the terminal

A program that draws its own screen and reads keys takes the terminal with
`cli.Terminal()`, which fails where there is none.

```sh
tondo init
rm src/main_test.to
```

```tondo file=src/main.to
import std.cli
import std.console

type Menu = { options: Array[String], chosen: Int }

/// The menu after a key: the arrows move the mark, and stop at the ends.
fn press(menu: Menu, key: cli.Key): Menu {
    match key {
        cli.Key.Up if menu.chosen > 0 => menu with { chosen: menu.chosen - 1 }
        cli.Key.Down if menu.chosen < menu.options.length() => menu with { chosen: menu.chosen + 1 }
        _ => menu
    }
}

/// The menu as a screen of that size.
fn draw(menu: Menu, size: cli.Size): String {
    let lines = for index in 0..menu.options.length() {
        "{if index == menu.chosen { ">" } else { " " }} {menu.options[index]}"
    }
    cli.Pane(lines.join("\n"), title: "Drinks", border: cli.Border.Rounded).draw(size)
}

fn main(): !cli.TerminalError {
    var menu = Menu { options: ["tea", "coffee", "water"], chosen: 0 }
    let terminal = cli.Terminal(screen: true)?
    let keys = terminal.keys()
    let size = cli.terminalSize() ?? cli.Size { columns: 24, rows: 5 }
    console.print(draw(menu, size))
    for {
        match keys.receive() {
            some(cli.Key.Enter) | none => break
            some(key) => menu = press(menu, key)
        }
        console.print(draw(menu, size))
    }
    terminal.release()
    console.println("a {menu.options[menu.chosen]}")
}
```

```mcp
run {}
=> the program's input is not a terminal
```

That is as far as `run` goes, and it is enough, because of how the program is
written: `main` only joins the terminal to two functions that need none.
`press` gives the menu after a key, and a key is a value like any other;
`draw` gives the screen as text, at a size it is told. Write a program of the
terminal this way, and every part of it has a test.

```file src/main_test.to
import std.cli
import std.testing

fn drinks(): Menu {
    Menu { options: ["tea", "coffee", "water"], chosen: 0 }
}

test theMarkMoves {
    let menu = press(press(drinks(), cli.Key.Down), cli.Key.Down)
    assert(menu.chosen == 2)
    assert(press(menu, cli.Key.Up).chosen == 1)
}

test theMarkStopsAtTheEnds {
    assert(press(drinks(), cli.Key.Up).chosen == 0)
    var menu = drinks()
    for _ in 0..5 {
        menu = press(menu, cli.Key.Down)
    }
    assert(menu.chosen == 2)
}
```

```mcp
test {}
=> theMarkMoves ... ok
=> theMarkStopsAtTheEnds ... FAILED
=> left:  3
```

Down goes one past the last option, where Enter would then panic: a bug of
the keys, found with no key pressed.

```edit src/main.to
-        cli.Key.Down if menu.chosen < menu.options.length() => menu with { chosen: menu.chosen + 1 }
+        cli.Key.Down if menu.chosen < menu.options.length() - 1 => menu with { chosen: menu.chosen + 1 }
```

## The screen, as a snapshot

What `std.cli` draws is text of the width it is given, with no codes where the
output is no terminal, so a screen is compared as any text. A snapshot keeps
it in a file, which a review reads as the screen itself.

```edit src/main_test.to
-test theMarkMoves {
+test drawsTheMenu {
+    let menu = press(drinks(), cli.Key.Down)
+    testing.snapshot("menu", draw(menu, cli.Size { columns: 24, rows: 5 }))
+}
+
+test theMarkMoves {
```

`update_snapshots` records it, in a run where every test passes, and from
then on `test` compares.

```mcp
test {"update_snapshots": true}
=> 3 passed
test {}
=> drawsTheMenu ... ok
```

```text
╭─ Drinks ─────────────╮
│   tea                │
│ > coffee             │
│   water              │
╰──────────────────────╯
```

That is `snapshots/unit/runbook_terminal/drawsTheMenu/menu.snap`. Read what a
snapshot records before keeping it: it holds whatever was drawn, right or not.

## What only a terminal shows

How it feels, and what a real terminal sends, takes a person at one. Ask for
the run to be recorded there:

```text
tondo run --record menu.rec
```

The recording keeps the keys that were pressed and the sizes the terminal
had, with everything else the program read. `debug_start` with `replay` and
that directory runs it again here, with no terminal: the screens it drew come
as its output, a panic stops where it started, and `back`, `why` and
`timeline` go through it: `runbook-once`.

A program suspended with Ctrl-Z, a terminal that changes its size and a panic
on the alternate screen give the terminal back as it was, by themselves:
there is nothing of that to test.
