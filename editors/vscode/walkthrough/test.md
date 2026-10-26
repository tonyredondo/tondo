# Test

The Testing view shows every test and suite of the workspace's projects,
and the examples of its Markdown documents. A failure shows its message on
the line that failed, a comparison as a diff, and its stack.

- *Run Tests Continuously* runs again the tests a saved change can affect.
- *Run with Coverage* shows the lines, functions and branches tests ran.
- *Benchmark* measures the tests that call `testing.bench`, and opens what
  they measured beside the code.
- *Debug Test* runs one test alone under the debugger.
- A snapshot that differs offers *Update Snapshot*.

`tondo.test.args` and `tondo.test.env` add arguments and variables.
