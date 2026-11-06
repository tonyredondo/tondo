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

Every step of these runbooks runs in the tests of the toolchain, so what they
show is what the tools give. A block of tool calls reads `tool {arguments}`,
then `=>` and what its result says. `<dump>`, `<pid>` and `<session>` stand
for the file, the process and the session of the steps before.

A result that leads somewhere says so: `next` names the tool that goes on from
it, and the runbook.
