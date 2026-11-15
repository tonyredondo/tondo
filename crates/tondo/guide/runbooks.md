# Runbooks

What to do when something goes wrong, step by step, with the tools of
`tondo mcp`: why each step, and what it gives. Read the one that fits; each
is a topic of the guide.

| What you see | Topic |
| --- | --- |
| A program panics or crashes, or you have a dump of one | `runbook-crash` |
| A failure must not come back: a dump to a test that fails | `runbook-regression` |
| A program or a test does not end or does not answer | `runbook-hang` |
| You change a running program, without starting it again | `runbook-live` |
| A program does not compile | `runbook-compile` |
| A test fails sometimes, or tasks race | `runbook-flaky` |
| A value is wrong and nothing says where | `runbook-debug` |
| A request of a server is slow | `runbook-slow-request` |
| A program is slow, or a change may have made it so | `runbook-performance` |
| Memory keeps growing | `runbook-memory` |
| Failures come from many machines, or must be shared | `runbook-production` |
| You want to find the inputs that break a function | `runbook-fuzz` |
| You change code and must show it works: which tests, alone and together | `runbook-testing` |
| Code allocates more than it should | `runbook-allocations` |
| A failure happened once, and running it again does not bring it back | `runbook-once` |
| It worked before: which commit broke it | `runbook-bisect` |
| A test fails in CI and not here, or a job must leave what it takes to see why | `runbook-ci` |
| You rename, change a signature or delete what other code uses | `runbook-change` |
| A program runs and you want to know what it is doing, without stopping it | `runbook-running` |
| A program asks, draws or reads keys, and there is no terminal to try it on | `runbook-terminal` |
| You port code from another language: the method | `runbook-port` |
| You port code from Go | `runbook-port-go` |
| You port code from C# | `runbook-port-csharp` |
| You port code from Rust | `runbook-port-rust` |
| You port code from Python | `runbook-port-python` |
| You port code from TypeScript or JavaScript | `runbook-port-typescript` |
| You port code from Java | `runbook-port-java` |
| You port code from C++ | `runbook-port-cpp` |
| You port code from Swift | `runbook-port-swift` |

Every step of these runbooks runs in the tests of the toolchain, so what they
show is what the tools give. A block of tool calls reads `tool {arguments}`,
then `=>` and what its result says. `<dump>`, `<pid>` and `<session>` stand
for the file, the process and the session of the steps before. A block marked
`linux` is of what only Linux has, as a logpoint.

A result that leads somewhere says so: `next` names the tool that goes on from
it, and the runbook.
