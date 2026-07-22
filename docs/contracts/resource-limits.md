# Bootstrap resource limits

**Status:** accepted defaults for implementation version 0.0.x

These limits defend the compiler from untrusted inputs. They are implementation
budgets, not Tondo language semantics. Embedding hosts may construct a request
with different explicit limits; the CLI uses the defaults.

| Resource | Default |
|---|---:|
| Bytes per source file | 64 MiB |
| Source files per request | 65,536 |
| Lossless syntax tokens per request | 2,000,000 |
| Lossless syntax nodes per request | 4,000,000 |
| Syntax nesting depth | 256 |
| Interned type nodes | 4,000,000 |
| Typed HIR expression and pattern nodes | 4,000,000 |
| Pattern-analysis matrix work | 4,000,000 |
| MIR functions | 100,000 |
| MIR blocks per function | 1,000,000 |
| MIR locals per function | 1,000,000 |
| MIR statements per function | 4,000,000 |
| MIR verification dataflow work | 32,000,000 |
| Bytecode type entries | 4,000,000 |
| Bytecode nominal/callable/constant entries | 1,000,000 each |
| Bytecode functions | 100,000 |
| Bytecode slots and blocks per function | 1,000,000 each |
| Bytecode instructions and spans per function | 4,000,000 each |
| Bytecode verification dataflow work | 32,000,000 |
| VM executed instructions | 100,000,000 |
| VM frame depth | 65,536 |
| VM live heap objects | 1,000,000 |
| VM live heap bytes | 1 GiB |
| VM initial collection threshold | 1,024 objects |
| Generic instantiations | 1,000,000 |
| Trait obligations | 1,000,000 |
| Primary diagnostics | 10,000 |
| Rendered diagnostics JSON | 64 MiB |

When a limit is reached, the compiler must stop the affected expansion with an
implementation resource diagnostic. It must not panic, wrap a counter, silently
truncate valid output, or reinterpret the program.

The formatter runs only after the complete frontend request has passed these
checks. A syntax or resource rejection therefore returns no partial canonical
source on command stdout.

Source count and bytes are enforced before the frontend. The lexer enforces the
request-wide token and primary-diagnostic budgets while scanning, and enforces
the nesting budget for nested comments and interpolations. The parser enforces
request-wide syntax-node and remaining primary-diagnostic budgets plus the same
nesting budget. Type lowering bounds canonical type nodes, and expression
checking shares one typed-HIR budget between expression and pattern arenas.
Pattern usefulness, reachability, and exhaustiveness share a separate
matrix-work budget and use an explicit worklist rather than the process stack.
Every generic-bound proof attempt consumes the trait-obligation budget. The
same configured ceiling independently bounds size-change termination work:
matrix cells, structural-subterm traversal, matrix composition, idempotence
checks, and diagnostic-witness expansion. The common proof for `Copy`,
`Discard`, `Equatable`, `Key`, `Send`, and `Share` computes finite symbolic
summaries over the already bounded interned type graph and does not recursively
instantiate nominal families. Each generic
bound request still consumes the trait-obligation budget. Concrete closure
signatures and environments are bounded by the existing syntax, type, and HIR
budgets. Callable-protocol derivation performs a bounded traversal over the
already topological HIR arenas and does not add recursive source traversal.
Ownership availability traverses those same bounded arenas; each loop state
grows monotonically over the finite local table and therefore reaches a fixed
point without an open-ended runtime heuristic. Source nesting remains bounded
by the parser's process-safety ceiling.

MIR and bytecode construction bound every request-local table before growth;
their initialization, lifetime, and tag-refinement analyses share independent
step budgets and use worklists. Bytecode monomorphization counts each unique
named or closure callable plus concrete argument vector exactly once; a generic
closure body therefore consumes one instance independently of its enclosing
function. Same-instance recursion is deduplicated, while type-expanding
recursion stops at the generic instantiation limit. Closure callable schemas and
protocol rows use the existing callable/type table budgets. Specialized type
construction remains subject to the interned type-node limit, and the final
concrete catalog has its own bytecode type-entry limit.

VM admission and execution enforce non-zero verification, instruction, frame,
object, byte, and initial-collection budgets. The collector performs a full
collection before reporting heap exhaustion. Bootstrap resource exhaustion uses
diagnostic code `T0002`.

The handwritten parser also clamps an embedding host's requested nesting depth
to 256. This is a process-safety ceiling for the recursive bootstrap parser,
not a semantic nesting rule of the Tondo language.
