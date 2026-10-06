# Contrato de `std.regex`

Estado: **contract-locked** para `STD-0.1B` / `STD-REGEX-001`.

This document fixes the normative `std.regex` boundary. `STD-REGEX-IMPL-001`
verifies the bounded Rust kernel described in section 10. Public Tondo
compiler/VM registration, native ABI and native AOT are not promoted.
The independent model/fuzz boundary is defined by
[stdlib-regex-test.md](./stdlib-regex-test.md) and
`testing/stdlib-regex-test.json`. Its verified scope includes the independent
bounded oracle, 41 valid/33 invalid retained vectors, 4,096 generated seeds and
128 seeded fuzz runs. The middle-hyphen correction and testing boundary pass
their joint quality and functional proof. The target-qualified direct Rust
performance boundary is described in
[stdlib-regex-performance.md](./stdlib-regex-performance.md) and
`testing/stdlib-regex-performance.json`. Its `verified-stdlib-kernel-baseline`
state follows the clean-source campaign and the full functional and quality
gates. The shared conformance protocol is described in
[stdlib-regex-conformance.md](./stdlib-regex-conformance.md) and
`testing/stdlib-regex-conformance.json`. Its
`verified-hosted-vm-adapter-and-native-stdlib-process` state covers seven case
groups after the clean comparison and source-bound quality/full gates pass.
It uses private VM dispatch and a Rust process, with no public regex VM API,
native ABI or Tondo AOT promotion. The `verified-rust-kernel-usage` documentation
record for `STD-REGEX-DOC-001` points to the executable guide below and its
canonical Rust example. The canonical execution, documentation regressions, full functional
gate and current source-bound quality checks pass. Publication and exact-SHA CI
closure are tracked separately in the live tracker.

La API es deliberadamente una sola superficie: compilar una expresión produce
un valor inmutable y reutilizable; las operaciones de búsqueda son puras,
acotadas y síncronas. No hay una API async paralela, un global de locale, un
registro dinámico de engines ni una capability implícita.

## 1. Alcance y objetivos

`std.regex` ofrece expresiones regulares sobre `String` Tondo válido. El owner
debe proporcionar:

1. compilación verificable de un patrón;
2. búsqueda booleana, primera coincidencia y enumeración no solapada;
3. captures posicionales y con nombre;
4. reemplazo literal con referencias de captures;
5. semántica Unicode estable y sin locale;
6. límites explícitos de patrón, programa, input, pasos, matches y output; y
7. errores nominales con offsets byte-exactos y ningún resultado parcial.

El motor debe ser apto para datos no confiables. Una entrada válida nunca puede
activar backtracking exponencial oculto, recursión ilimitada del host, consumo
sin presupuesto o un callback ejecutable desde el patrón.

El contrato se inspira en el soporte Unicode básico de
[UTS #18](https://www.unicode.org/reports/tr18/tr18-21.html) y en engines de
autómatas finitos. No pretende ser un dialecto Perl ni una implementación de
todos los niveles de Unicode. El nivel y las exclusiones son parte de este
contrato, no detalles de una implementación.

## 2. Unicode y texto

### 2.1 Unidad de matching

- El input y el patrón son `String`, UTF-8 válido por construcción.
- La unidad lógica es un scalar Unicode, nunca un byte ni una unidad UTF-16.
- Los offsets públicos son bytes UTF-8 half-open `[start, end)` y siempre caen
  en límites de scalar.
- `RegexSpan.slice` rechaza un span fuera de rango o que corte un scalar.
- No se hace NFC, NFD, NFKC, NFKD ni canonical equivalence automática.
- Los grapheme clusters extendidos no son una unidad de matching en 0.1; un
  emoji con modificador o ZWJ puede producir varias coincidencias escalares.
- No se consulta locale, `LANG`, `LC_*`, timezone ni environment.

Las tablas de propiedades y de simple case folding son exactamente las de
Unicode **16.0.0**, la misma versión fijada por la especificación del lenguaje.
Un target no puede sustituirlas silenciosamente. Un artefacto compilado registra
la versión de Unicode y una conformance VM/nativa debe usar el mismo descriptor.

### 2.2 Propiedades

Se soportan propiedades Unicode con la forma `\p{Name}` y su complemento
`\P{Name}`. El catálogo se deriva de las tablas UCD 16.0.0 y admite:

- General Category y sus abreviaturas (`L`, `Letter`, `Lu`, `Nd`, etc.);
- `Script` y `Script_Extensions` con nombres canónicos UCD; y
- propiedades binarias cerradas (`Alphabetic`, `White_Space`, `XID_Start`,
  `XID_Continue`, `Emoji` y las demás propiedades binarias publicadas por el
  descriptor del target).

Un nombre desconocido, alias ambiguo o propiedad no disponible en el descriptor
produce `InvalidUnicodeProperty` durante `compile`; nunca se interpreta como
una clase vacía.

Las clases abreviadas tienen este significado estable:

| Escape | Clase |
|---|---|
| `\\d` / `\\D` | `Decimal_Number` / su complemento |
| `\\s` / `\\S` | `White_Space` / su complemento |
| `\\w` / `\\W` | `Alphabetic ∪ Mark ∪ Decimal_Number ∪ Connector_Punctuation` / su complemento |
| `\\b` | frontera entre `\\w` y `\\W`, también al inicio/final |

`\b` no consume texto. En modo Unicode no existe una opción ASCII que cambie
estos significados; si se necesita una clase ASCII se escribe explícitamente.

### 2.3 Case folding y líneas

`RegexOptions.caseInsensitive` usa simple case folding Unicode 16.0.0, sin
expansiones de varios scalars. Por tanto, una comparación no puede convertir un
scalar en dos o más scalars y los offsets permanecen estables.

`^` y `$` son anclas de input completo por defecto. Con
`RegexOptions.multiLine`, también se sitúan después/antes de `LF`; si
`RegexOptions.crlf` está habilitado, la pareja `CRLF` se trata como una única
frontera de línea. `\\A` y `\\z` siempre son anclas absolutas. `.` coincide con
cualquier scalar salvo `LF` (y `CR` cuando `crlf` está habilitado);
`dotMatchesNewline` elimina esa exclusión.

## 3. Sintaxis cerrada

La gramática abstracta es:

```text
pattern       := alternation
alternation   := sequence ("|" sequence)*
sequence      := (atom quantifier?)*
atom          := literal | "." | class | group | anchor
group         := "(" alternation ")"
                | "(?:" alternation ")"
                | "(?<name>" alternation ")"
class         := "[" "^"? class_item+ "]"
class_item    := scalar | scalar "-" scalar | escape | property
escape        := "\\" ("\\" | "." | "^" | "$" | "|" | "(" | ")"
                | "[" | "]" | "{" | "}" | "*" | "+" | "?" | "-"
                | "a" | "f" | "n" | "r" | "t" | "v"
                | "d" | "D" | "s" | "S" | "w" | "W" | "b"
                | "A" | "z" | "p" | "P" | "x")
property      := "\p{" property_name "}" | "\P{" property_name "}"
quantifier    := ("*" | "+" | "?" | "{m}" | "{m,n}" | "{m,}") "?"?
```

Detalles normativos:

- Un literal metacarácter debe escaparse. Un escape desconocido es un error,
  no un literal tolerante.
- `\\x{HEX}` admite de uno a seis dígitos y debe designar un scalar Unicode;
  surrogates y valores mayores que `U+10FFFF` se rechazan.
- Los escapes `\\a`, `\\f`, `\\n`, `\\r`, `\\t` y `\\v` producen sus scalars
  ASCII correspondientes.
- Un rango de clase debe estar en orden escalar ascendente. El guion literal se
  escribe primero, último o escapado.
- `[]`, `[^]`, rangos con extremos inválidos, una clase sin cierre o una clase
  que mezcla una propiedad negada de forma ambigua producen `InvalidClass`.
- `m`, `n` son enteros decimales sin signo, `m <= n`; `{m,}` es abierto pero
  está sujeto a `max_repeat` y `max_steps`.
- Los quantifiers se aplican a un único atom. Un quantifier duplicado,
  `{0,0}` sobre una atom vacía o un overflow del contador producen
  `InvalidQuantifier`.
- Los quantifiers son greedy por defecto. `RegexOptions.ungreedy` invierte esa
  preferencia. La forma sufija lazy (`*?`, `+?`, `??`, `{m,n}?`) también está
  permitida; no existen quantifiers possessive.
- `(...)` crea un capture numerado según el paréntesis de apertura; `(?:...)`
  no captura; `(?<name>...)` crea un capture numerado y con nombre.
- Los nombres de capture son ASCII `[_A-Za-z][_A-Za-z0-9]*`, únicos por
  patrón, y no pueden coincidir con un alias de propiedad ambiguo.
- El máximo de profundidad sintáctica, captures, clases, alternancias y
  repeticiones se valida antes de construir el programa.

### 3.1 Features fuera del dialecto

Se rechazan de forma explícita:

- backreferences (`\\1`, `\\k<name>`);
- look-ahead y look-behind (`(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)`);
- grupos atómicos o possessive, recursion, subroutine calls y conditionals;
- código, callouts, interpolación, macros, variables o includes;
- `\\C` y cualquier operación sobre bytes potencialmente UTF-8 inválidos;
- clases POSIX dependientes del locale;
- `(?i)` o flags embebidos que oculten una policy global; y
- sintaxis no listada en la gramática, aunque otro engine la acepte.

El error es `UnsupportedFeature` con el span del constructo. El patrón no se
compila parcialmente ni se degrada a una interpretación diferente.

## 4. Modelo de ejecución

### 4.1 Programa compilado

`Regex` es un valor inmutable, shareable y reusable entre llamadas. Contiene el
patrón original, las opciones, el descriptor Unicode y un programa compilado;
ninguna de esas piezas observa el environment después de `compile`.

El engine debe usar Thompson NFA, lazy DFA, tagged automata o una técnica
equivalente con prueba de complejidad lineal. Puede cambiar de representación
por tamaño/target, pero no puede introducir una ruta de backtracking sin un
presupuesto declarado y sin preservar la misma semántica. La pila del host no
se usa para recorrer el árbol del patrón: parser, worklists, estados y captures
son estructuras explícitas y acotadas.

`compile` es atómico: ante cualquier error no devuelve `Regex`. El fingerprint
de compile es determinista para `(pattern, options, limits, unicode_version)`;
no incluye direcciones, timestamps ni hashes de memoria del host.

### 4.2 Selección de coincidencias

Todas las operaciones de búsqueda siguen la misma regla:

1. se elige el inicio más a la izquierda en scalars/bytes del input;
2. alternatives retain their order in the pattern;
3. each quantifier applies local greedy/lazy priorities, inverted by `ungreedy`;
4. captures follow those same priorities deterministically.

For example, `a|ab` over `ab` selects `a`; `a*?a*` over `aaa` selects `aaa`.
Selection does not impose a global longest or shortest match rule.

`isMatch` busca en cualquier posición. `isFullMatch` exige que la coincidencia
abarque exactamente todo el input. `match` devuelve la primera coincidencia con
captures o `none`. `findAll` produce coincidencias no solapadas en orden. Si una
coincidencia tiene longitud cero, el siguiente intento avanza exactamente un
scalar Unicode; así un patrón vacío siempre termina y no produce duplicados.

`RegexFindIterator` conserva solo el cursor, los offsets y los captures del
último match. Presta el input mientras vive el iterador, no copia el texto y no
puede sobrevivir al owner del input. Consumirlo con `for` usa el protocolo
`Iterator[RegexMatch ! RegexError]`. It is lazy: each step produces a complete
match or one nominal error, after which the cursor returns `none` permanently.
Previously emitted matches remain valid. No `AsyncIterator` or `selectable`
operation is provided. Step and match budgets are cumulative across the cursor.

### 4.3 Captures y spans

`RegexMatch.span` cubre la coincidencia completa. Cada capture puede estar
ausente (`none`) si su rama opcional no participó; un capture vacío participante
se representa con `start == end`. Los índices son cero-based y el capture `0`
es siempre el match completo.

Los nombres son aliases del índice, no un segundo almacenamiento. Una consulta
por índice fuera de rango o por nombre desconocido devuelve `none`, no un error.
`RegexSpan.slice(input)` hace una copia `String` validada; no entrega una vista
que pueda escapar al input.

## 5. Reemplazo

`replace` sustituye la primera coincidencia y `replaceAll` todas las coincidencias
no solapadas. La plantilla de replacement admite:

- `$0` para el match completo;
- `$1` … `$N` para captures numerados;
- `${name}` para captures con nombre; y
- `$$` para un `$` literal.

Una referencia a un capture que no existe, un cierre `${` ausente o un escape
desconocido produce `InvalidReplacement` antes de modificar el output. Un
capture válido pero no participante inserta `""`. El output se construye en un
builder acotado; si supera `max_output_bytes` se devuelve
`OutputLimitExceeded` sin entregar un prefijo parcial.

No hay callback de replacement en esta primera edición: ejecutar una función
por cada match ocultaría coste, reentrancia y ownership. Una futura extensión
debe ser un contrato separado.

## 6. API pública

El módulo no requiere capability y no posee funciones top-level con efectos.
Las firmas normativas son:

```tondo
pub type Regex
pub type RegexOptions
pub type RegexLimits
pub type RegexErrorKind
pub type RegexError
pub type RegexSpan
pub type RegexCapture
pub type RegexMatch
pub type RegexFindIterator

pub fn RegexOptions.defaults(): RegexOptions
pub fn RegexLimits.defaults(): RegexLimits
pub fn Regex.compile(pattern: String, options: RegexOptions, limits: RegexLimits): Regex ! RegexError
pub fn Regex.pattern(self): String
pub fn Regex.captureCount(self): Int
pub fn Regex.captureNames(self): Array[String]
pub fn Regex.isMatch(self, input: String): Bool ! RegexError
pub fn Regex.isFullMatch(self, input: String): Bool ! RegexError
pub fn Regex.match(self, input: String): RegexMatch? ! RegexError
pub fn Regex.findAll(self, input: String): RegexFindIterator ! RegexError
pub fn Regex.replace(self, input: String, replacement: String): String ! RegexError
pub fn Regex.replaceAll(self, input: String, replacement: String): String ! RegexError
pub fn RegexFindIterator.next(var self): (RegexMatch ! RegexError)?
pub fn RegexMatch.capture(self, index: Int): RegexCapture?
pub fn RegexMatch.captureName(self, name: String): RegexCapture?
pub fn RegexSpan.slice(self, input: String): String ! RegexError
```

No firma suspende: todas son puras y bounded por `RegexLimits` fijados en el
valor compilado. Por el modelo async de Tondo, no se escribe `await` implícito
ni existe `Regex.*Async`; `selectable_operations` es vacío.

### 6.1 Opciones y límites

`RegexOptions.defaults()` produce `caseInsensitive = false`,
`multiLine = false`, `dotMatchesNewline = false`, `crlf = false` y
`ungreedy = false`. Unicode siempre está activo y no se puede desactivar.

`RegexLimits.defaults()` fija, como mínimo, estos límites observables:

| Límite | Unidad | Default | Se comprueba |
|---|---:|---:|---|
| `max_pattern_bytes` | bytes UTF-8 | 65536 | antes de parsear |
| `max_syntax_depth` | frames | 256 | antes de abrir grupo/clase |
| `max_capture_groups` | groups | 256 | antes de registrar capture |
| `max_class_ranges` | ranges | 262144 | antes de insertar rango |
| `max_repeat` | repetitions | 1000000 | antes de expandir/analizar |
| `max_program_states` | states | 1000000 | antes de publicar `Regex` |
| `max_input_bytes` | bytes UTF-8 | 16777216 | antes de buscar |
| `max_steps` | state transitions | 100000000 | antes de cada batch |
| `max_matches` | matches | 1000000 | antes de emitir |
| `max_output_bytes` | bytes UTF-8 | 67108864 | antes de append |
| `max_replacement_bytes` | bytes UTF-8 | 65536 | antes de parsear plantilla |
| `vm_heap` | runtime-defined | target | antes de publicar |

Todos los límites son positivos y no pueden desbordar `Int`. Reducirlos solo
puede producir un error nominal más temprano; nunca cambia la semántica de una
operación que termina dentro del presupuesto.

## 7. Errores y terminalidad

`RegexError` contiene `kind`, `phase` (`Compile`, `Match` o `Replace`),
`offset` byte half-open en patrón o input, `span` opcional y un descriptor
estable de límite. El texto humano no forma parte de la identidad del error.

Las variantes cerradas de `RegexErrorKind` son:

```text
InvalidSyntax
UnexpectedEnd
InvalidEscape
InvalidUnicodeScalar
InvalidUnicodeProperty
InvalidClass
InvalidRange
InvalidQuantifier
InvalidCaptureName
DuplicateCaptureName
UnsupportedFeature
PatternLimitExceeded
ProgramLimitExceeded
InputLimitExceeded
StepLimitExceeded
MatchLimitExceeded
ReplacementLimitExceeded
InvalidReplacement
OutputLimitExceeded
InvalidBoundary
OutOfMemory
NoProgress
```

La API no hace panic por input del usuario. Un límite, error de sintaxis,
replacement inválido o falta de memoria no publica `Regex`, `RegexMatch`,
iterator ni output parcial. Atomicity is per element for the lazy iterator:
an error preserves earlier complete matches and makes the cursor terminal.
Other pure operations have no terminal state between calls. An exhausted
`RegexFindIterator` returns `none` forever.

## 8. Rendimiento y seguridad

- A single search has linear input work for a fixed compiled program within
  `max_steps`; no hay backtracking exponencial. Lazy enumeration and repeated
  replacement can rescan remaining suffixes. Their cumulative work stays
  step-bounded; no total linear enumeration guarantee is claimed.
- La compilación puede usar lazy DFA o tablas equivalentes, pero cada estado,
  rango y transición cuenta contra `max_program_states`/`max_class_ranges`.
- El compilador y el matcher usan worklists explícitos; un patrón anidado no
  puede consumir la pila recursiva del proceso.
- Las propiedades Unicode usan tablas versionadas y lookup acotado.
- ASCII, literales y clases compactas pueden tener fast paths. SIMD está
  permitido únicamente tras demostrar equivalencia exacta con un oracle scalar,
  incluidos captures, boundaries, invalid UTF-8 imposible y case folding.
- No se publican cifras de throughput, allocations, tail latency ni tamaño de
  automata antes de `STD-REGEX-PERF-001`.
- El fuzz oracle exige ausencia de panic, determinismo, límites respetados,
  spans válidos, terminación de patrones vacíos y equivalencia de
  `findAll`/`replaceAll` frente al modelo.

## 9. Conformance y exclusiones

La conformance mínima cubre parser, escapes, clases, propiedades Unicode 16.0.0,
anchors, quantifiers, alternation, captures, greediness, zero-width matches,
replacement, límites, determinismo y equivalencia VM/nativa. Los casos de
features rechazadas deben comprobar el kind y el span del error.

Quedan fuera de `STD-REGEX-001`:

- backreferences, look-around, recursion, conditionals y código embebido;
- grapheme clusters, canonical equivalence y locale-sensitive matching;
- parser de glob, shell quoting o replacement callbacks;
- regex sobre bytes inválidos o buffers no UTF-8;
- compilación JIT, captura de una ABI externa y serialización de `Regex`; y
- un engine configurable por módulo o por environment.

El contrato machine-readable es
[`testing/stdlib-regex.json`](../../testing/stdlib-regex.json). La integración
con la especificación principal y los checks ejecutables son
[`scripts/stdlib-regex-check.sh`](../../scripts/stdlib-regex-check.sh) y
[`scripts/stdlib-regex-test.sh`](../../scripts/stdlib-regex-test.sh).

## 10. Executable Rust kernel boundary

`STD-REGEX-IMPL-001` uses `regex-syntax =0.8.10` for its iterative parser and
Unicode 16.0.0 tables, with an ordered Thompson NFA implemented in Tondo's Rust
standard-library crate. The register distinguishes `ready-stdlib-kernel`
(functional checks ready, quality still pending) from `verified-stdlib-kernel`
(functional and workspace quality proof complete). Neither state promotes a
public compiler API, production VM registration, native ABI, native AOT,
SIMD, multiversion dispatch or measured performance.

The Rust kernel is in `crates/tondo-stdlib/src/regex.rs` and its `syntax` and
`engine` modules. `Regex::find` implements the specified `Regex.match` operation;
other method names use Rust snake case. `RegexCapture` is a span, captures are
indexed including capture zero, and `capture_count()` excludes capture zero.
`capture_names()` visits named captures in opening-parenthesis order. Matches
own their spans and share immutable name metadata. The iterator borrows both
the program and UTF-8 input; successful matches remain usable after it ends.

Closed-dialect admission counts syntax frames and captures before the dependency
constructs its AST. It checks repetition bounds and rejects unsupported syntax
before translation. AST and HIR traversal, program construction, thread closure
and destruction use explicit worklists. Each class is admitted separately before
translating the entire pattern. `\w`, `\W` and `\b` use Tondo's exact word set;
`Join_Control` is excluded even though the dependency's default word class
includes it. Property assignments are limited to General Category, Script and
Script Extensions. Bare General Category, Script and supported UCD binary names
use the pinned tables. Age, segmentation property assignments, and synthetic
`Any`, `ASCII` and `Assigned` properties are outside this descriptor.

Alternatives and repetition splits preserve local priorities. A successful
candidate suppresses lower-priority threads while higher-priority threads can
continue. Zero-progress loop exits retain the final empty capture without
re-entering an empty repetition. Visited state includes the active loop progress
depth; marking only the instruction would lose valid paths and empty captures.
Full matching checks the end condition during selection, so `a|ab` fully matches
`ab` through its second alternative even though ordinary search selects `a`.

Each search is linear in input length for a fixed compiled program, with bounded
thread width and capture snapshots. `findAll` and `replaceAll` may perform several
searches over overlapping suffixes: for example, `a*b|a` over repeated `a` can
rescan text. They have a cumulative `max_steps` budget, not a claim of total
linear enumeration time. The same budget covers failed attempts, duplicates
visited during closure and consuming transitions. The iterator emits one error
then terminal `none`; `replaceAll` discards its builder on any error. Every
replacement template is validated even when no match exists. Match budgets
check before emission and allow exact exhaustion when no further match exists.

With `multiLine` and `crlf`, anchors recognize LF and the complete CRLF pair,
never the position between CR and LF. A standalone CR is not a line boundary.
The dot excludes CR and LF with `crlf`, unless `dotMatchesNewline` is enabled.
`RegexSpan.slice` checks ordering, range and UTF-8 scalar boundaries before
returning an owned copy.

The Rust mapping of `vm_heap` defaults to 128 MiB of conservative logical storage.
Admission includes a 256-byte-per-pattern-byte parser estimate, materialized
Unicode ranges, program entries, visited progress states, thread/capture
snapshots and replacement buffers. This is not RSS, allocation-call counting,
or a production VM allocator guarantee. Fallible owned buffer reservations and
logical-budget exhaustion produce `OutOfMemory`; recovery from exhaustion in
the dependency or Rust global allocator is not claimed. No host allocator or
environment setting changes matching behavior.

Compile identity is SHA-256 over a versioned domain, Unicode version, exact
pattern bytes, the five option flags and all twelve limits encoded as little
endian u64 values. It excludes paths, pointer values, timing and hash-map seeds.
Functional proof covers syntax and rejection spans, captures, Unicode 16,
folding, anchors, local priorities, iterator lifecycle, replacements, limits,
fingerprints, a small-stack nested-pattern check and 4,096 deterministic
generated patterns. `STD-REGEX-TEST-001` separately verifies the independent
bounded grammar/path model and seeded fuzz; its Unicode properties use distinct
authored expectations, not a second full table oracle. Performance, shared
VM/native conformance and an executable usage guide remain their separate leaves.

Run `scripts/stdlib-regex-implementation-check.sh` and
`scripts/stdlib-regex-implementation-test.sh` for contract and kernel proof.
`scripts/stdlib-regex-implementation.sh` generates source-bound functional
evidence from the canonical tests and Clippy; it does not measure coverage.
Quality is established separately at the unchanged 80% global and per-scope
floor, with the critical mutation baseline preserved.

The implementation and independent testing leaves pass the full functional
gate, including the 206-case draft suite, async/select observations and the
128-run regex smoke. Their joint source-bound workspace campaign measures
291,951 of 318,803 lines (91.5772%) at the independent testing boundary;
every global and risk-scope line/function/region dimension meets the 80% floor,
and all six selected critical mutants are caught. These observations promote
`verified-stdlib-kernel` and the independent model/test/fuzz boundary only.
Public Tondo calls, production VM registration and native integration remain
unimplemented/unclaimed. At that testing boundary the next owner was
`STD-REGEX-PERF-001`. The later performance and shared conformance leaves are
now verified within their stated kernel/adapter boundaries. The current
workspace campaign covers 292,638 of 319,506 lines (91.5908%), meets every 80%
floor and catches all six critical mutants. The documentation leaf now verifies
the canonical executable guide within its Rust-kernel boundary. The next owner
after publication and exact-SHA CI closure is `STD-UUID-IMPL-001`.

## Executable usage guide for `std.regex`

This guide runs the bounded Rust kernel. The declarations above specify the
public Tondo API, whose compiler imports, production VM registration, native
regex ABI and native AOT lowering are unimplemented. Rust calls below do not
establish execution of those Tondo declarations. The kernel is synchronous;
it requires no locale, environment lookup, capability or async wrapper.

### Patterns and reuse

Compile with `Regex::compile(pattern, options, limits)` and keep the immutable
program for repeated searches. Start with `RegexOptions::default()` and
`RegexLimits::default()`. Literal strings and scalars match directly; use Rust
raw strings when backslashes should reach the regex parser unchanged, such as
`r"\p{Letter}+"`. Embedded flags, lookaround, backreferences and callbacks are
outside the closed dialect. Select supported flags through `RegexOptions`.

`is_match` asks whether any substring matches. `find` returns the first match
by leftmost start and local alternative/quantifier priorities. `is_full_match`
requires the entire input before it selects an acceptable path; it is not a
comparison of the first search span with the input length. Thus `a|ab` finds
`a` in `ab`, while full matching succeeds with `ab`. Local lazy behavior also
means `a*?a*` can select all of `aaa`.

The fingerprint includes the exact pattern, options, limits and Unicode
version. Equal inputs have equal fingerprints; changing a budget changes that
identity. This does not create an implicit global cache or promise a portable
serialized compiled program. Rust `Regex` has no `Clone` method; share immutable
access with ordinary references or `Arc` when appropriate.

### Unicode and options

Both pattern and input are valid UTF-8 strings. Matching operates on scalars,
and returned spans use UTF-8 byte offsets. Unicode properties and simple case
folding use version 16.0.0. Kelvin sign can match `k` with
`case_insensitive`, but sharp-s does not expand into `ss`. There is no locale
lookup, normalization or grapheme-cluster matching: precomposed `é` and
`e` followed by a combining accent remain different sequences.

The five flags are `case_insensitive`, `multi_line`, `dot_matches_newline`,
`crlf` and `ungreedy`; all default to false. Unicode remains enabled.
`multi_line` changes the line anchors, `crlf` treats CRLF as one line boundary,
`dot_matches_newline` changes dot admission, and `ungreedy` reverses local
greedy/lazy preferences. Absolute `\A` and `\z` still refer to the whole input.

### Captures and UTF-8 spans

Capture zero is the whole match. Use `capture(index)` for a numbered capture or
`capture_name(name)` for a named capture. An absent optional group returns
`None`; a participating empty group returns a span with equal start and end.
The last participating repetition supplies the retained capture. A capture
name lookup which does not exist also returns `None` in this Rust kernel.

`RegexSpan { start, end }` describes half-open byte offsets. For `é`, the whole
span is `0..2`, not `0..1`. `RegexSpan::slice` validates range and scalar
boundaries, then returns an owned String copy. Slicing `1..2` fails with
`InvalidBoundary`. Keep and supply the corresponding original input when
interpreting spans; a span does not store that text or its identity.

### Lazy iteration and ownership

`find_all` lends both the compiled regex and the input for the cursor's
lifetime. It does not materialize every match in advance. Each Rust iterator
item is `Result<RegexMatch, RegexError>`; the public Tondo protocol specified
above is `Iterator[RegexMatch ! RegexError]`.

The cursor is non-overlapping and carries a cumulative step budget. Empty
matches advance by one Unicode scalar and visit EOF once. On `é🙂`, an empty
pattern produces byte offsets 0, 2 and 6. Exhaustion stays exhausted.
An error is delivered once and the cursor then returns `None` permanently.
Already delivered owned matches remain valid; retain the corresponding input
to read their numeric spans after the cursor's scope ends. A later independent
search receives a fresh budget.

Choose how to handle a fallible item explicitly. Collecting into
`Result<Vec<_>, _>` stops on the first error and does not return the earlier
vector. The executable example instead retains a delivered match before a
`MatchLimitExceeded` at byte 1 and verifies the terminal cursor twice.

### Replacement and errors

`replace` changes the first match; `replace_all` changes every non-overlapping
match. Templates support `$0`, numbered `$1` captures, `${name}` and literal
`$$`. A valid absent capture inserts empty text. Unknown references and
malformed templates produce `InvalidReplacement` even when the input has no
match. Replacement does not execute callbacks or interpret arbitrary code.

Zero-width replacement follows the same scalar progress, preserving input
between insertions: replacing empty matches in `é🙂` with `_` yields
`_é_🙂_`. The returned String is atomic. `OutputLimitExceeded` returns no
partial owned output, and the regex can be reused afterward. There is no sink
or external I/O transaction in these pure text operations.

`RegexError` records a nominal kind, `Compile`, `Match` or `Replace` phase,
byte offset, optional byte span and optional limit descriptor. Use those fields
instead of matching human error text. The example verifies the middle-hyphen
class refusal at bytes `6..7` in `é[a-b-c]`.

### Limits and costs

Set finite positive limits for the application's input. Pattern/input/output/
template budgets count UTF-8 bytes; other fields bound syntax depth, captures,
class ranges, repetitions, program states, transitions and emitted matches.
Configuration values outside the positive Tondo Int range are rejected during
compile. The Rust kernel reports `PatternLimitExceeded` with the offending
limit descriptor even for an invalid zero `max_steps` configuration. A valid
step budget exhausted during matching instead yields `StepLimitExceeded`.

`vm_heap` is a selected conservative logical storage admission budget, not
RSS or a VM allocator counter. It does not measure every parser temporary,
thread workspace or Rust allocation, and it does not guarantee recovery from
system OOM in the parser dependency or global allocator. `find` owns capture
storage; name metadata is shared, and `RegexSpan::slice` copies text.

A single ordered NFA search is bounded by program size, input and `max_steps`.
Iteration and replacement can rescan suffixes for repeated searches, so their
total cost is not claimed globally linear in input length. They share a
cumulative transition budget and should use suitable match/output limits.
The [performance contract](./stdlib-regex-performance.md) measures nineteen
bounded Rust test-profile routes with 27 retained samples per route. These
unoptimized kernel timings and selected logical resources do not measure
production Tondo AOT, native regex handles, RSS, SIMD or code size.

### Executable kernel example

[regex_usage.rs](../../crates/tondo-stdlib/examples/regex_usage.rs) checks six
usage paths: patterns and reuse; Unicode and options; captures and byte spans;
lazy iteration and ownership; replacement and errors; limits and costs.
`scripts/stdlib-regex-doc-check.sh` compiles and executes this one canonical
source with the workspace lockfile. The example's entire successful stdout is
`regex-doc-ok`. No alternate source override substitutes another example.

### Promotion boundary

`STD-REGEX-DOC-001` documents executable Rust-kernel usage. The independent
model, bounded fuzz, target-qualified performance and private VM/native Rust
process comparison retain their own measured boundaries. This guide does not
promote the public Tondo regex API, production host registration, a native regex
ABI, Cranelift lowering, SIMD or release readiness. The parent register records
`verified-rust-kernel-usage` after the canonical example, negative documentation
checks, full functional gate and source-bound quality pass. The next owner after
publication and exact-SHA CI closure is `STD-UUID-IMPL-001`.
