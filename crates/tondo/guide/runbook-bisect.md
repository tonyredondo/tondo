# Runbook: it worked before

Something that was right is wrong now, and the history is long. Write the test
that says what should hold, today, and `test_bisect` finds the first commit
where it fails: it tries the commits between a good one and this one, each in
a worktree of its own, with the tests as they are here.

```sh
tondo init
rm src/main_test.to
```

```tondo file=src/main.to
/// A price with its tax, in cents, rounded to the nearest one.
fn withTax(cents: Int): Int {
    cents + (cents * 21 + 50) / 100
}

fn main() {}
```

The history of this runbook: a release that was right, and three changes since.

```sh
git init -q
git config user.name Ada
git config user.email ada@example.com
git config commit.gpgsign false
git add .
git commit -q -m "Add the tax to a price"
git branch released
```

```edit src/main.to
-fn main() {}
+/// A price after a discount of so many percent.
+fn discounted(cents: Int, percent: Int): Int {
+    cents - cents * percent / 100
+}
+
+fn main() {}
```

```sh
git commit -q -a -m "Add discounts"
```

```edit src/main.to
-    cents + (cents * 21 + 50) / 100
+    cents + cents * 21 / 100
```

```sh
git commit -q -a -m "Simplify the tax"
```

```edit src/main.to
-fn main() {}
+/// What sending an order costs: nothing from 50 euros up.
+fn shipping(cents: Int): Int {
+    if cents >= 5000 { 0 } else { 495 }
+}
+
+fn main() {}
```

```sh
git commit -q -a -m "Add shipping"
```

A client was charged 3.02 for a price of 2.50, and 3.03 is right. First the
test that says so, which fails today: it need not be committed.

```file src/main_test.to
test roundsToTheNearestCent {
    assert(withTax(250) == 303)
}
```

```mcp
test {"filter": "roundsToTheNearestCent"}
=> roundsToTheNearestCent ... FAILED
```

`test_bisect` takes a commit where the test passes, here the release, and the
test. A commit that does not compile says nothing, and is passed over.

```mcp
test_bisect {"good": "released", "filter": "roundsToTheNearestCent"}
=> Simplify the tax
```

The commit is small, and its change is the cause: the half a cent that rounded
to the nearest one went with the simplification. Put it back, and keep the
test, which is what stops it from going again.

```edit src/main.to
-    cents + cents * 21 / 100
+    cents + (cents * 21 + 50) / 100
```

```mcp
test {}
=> roundsToTheNearestCent ... ok
```

For speed rather than a failure, `bench` with `max_regression` finds the first
commit where a benchmark got that many percent slower than at `good`, by time
or, with `regression_by` `instructions`, by what where the code lies does not
move. `test` with `affected` and a commit runs only the tests that reach what
changed since it, and `api_diff` says what a project's public API lost since
one.

With no good commit to start from, a failure in a dump becomes a test with
`dump_test`: `runbook-regression`.
