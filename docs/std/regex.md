# std.regex

Unicode regular expressions that run in time linear in their input, on the runtime's engine. No backreferences or look-around; spans are byte offsets into the input.

```tondo
import std.console
import std.regex

let digits = regex.Regex.compile(r"\d+")?
for found in digits.findAll("a1b22c333")? {
    console.println(found?.text)
}
console.println(digits.replaceAll("a1b22", "#")?)
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.regex` shows the same in a terminal.

- [RegexOptions](#regexoptions): [`RegexOptions.defaults`](#regexoptionsdefaults)
- [RegexLimits](#regexlimits): [`RegexLimits.defaults`](#regexlimitsdefaults)
- [RegexErrorKind](#regexerrorkind)
- [RegexPhase](#regexphase)
- [RegexError](#regexerror)
- [RegexSpan](#regexspan): [`RegexSpan.slice`](#regexspanslice)
- [RegexCapture](#regexcapture)
- [RegexMatch](#regexmatch): [`RegexMatch.capture`](#regexmatchcapture), [`RegexMatch.captureName`](#regexmatchcapturename)
- [Regex](#regex): [`Regex.compile`](#regexcompile), [`Regex.pattern`](#regexpattern), [`Regex.captureCount`](#regexcapturecount), [`Regex.captureNames`](#regexcapturenames), [`Regex.isMatch`](#regexismatch), [`Regex.isFullMatch`](#regexisfullmatch), [`Regex.find`](#regexfind), [`Regex.findAll`](#regexfindall), [`Regex.replace`](#regexreplace), [`Regex.replaceAll`](#regexreplaceall), [`Regex.replaceWith`](#regexreplacewith), [`Regex.replaceAllWith`](#regexreplaceallwith), [`Regex.escape`](#regexescape)
- [RegexFindIterator](#regexfinditerator)

### RegexOptions

```tondo
pub type RegexOptions = {
    caseInsensitive: Bool
    multiLine: Bool
    dotMatchesNewline: Bool
    crlf: Bool
    ungreedy: Bool
}
```

How a pattern matches.

```tondo
import std.console
import std.regex

var options = regex.RegexOptions.defaults()
options.caseInsensitive = true
options.multiLine = true
let heading = regex.Regex.compile(r"^# (\w+)$", options: options)?
for found in heading.findAll("# Intro\ntext\n# USAGE\n")? {
    console.println(found?.capture(1).map((title) { title.text }).unwrapOr(""))
}
```

#### RegexOptions.defaults

```tondo
pub fn RegexOptions.defaults(): RegexOptions
```

Every option off: case-sensitive, `^` and `$` at the ends of the text,
`.` not matching `\n`, and greedy repetition.

### RegexLimits

```tondo
pub type RegexLimits = {
    maxPatternBytes: Int
    maxSyntaxDepth: Int
    maxCaptureGroups: Int
    maxProgramStates: Int
    maxInputBytes: Int
    maxMatches: Int
    maxOutputBytes: Int
    maxReplacementBytes: Int
}
```

How much a pattern and its inputs may take.

```tondo
import std.console
import std.regex

var limits = regex.RegexLimits.defaults()
limits.maxInputBytes = 1024
let word = regex.Regex.compile(r"\w+", limits: limits)?
match word.isMatch("x".repeat(2048)) {
    ok(found) => console.println("{found}")
    err(error) => console.println("refused: {error}")
}
```

#### RegexLimits.defaults

```tondo
pub fn RegexLimits.defaults(): RegexLimits
```

The default limits: patterns of up to 64 KiB, inputs of up to 16 MiB and
a million matches.

### RegexErrorKind

```tondo
pub enum RegexErrorKind {
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
    MatchLimitExceeded
    ReplacementLimitExceeded
    InvalidReplacement
    OutputLimitExceeded
    InvalidBoundary
}
```

What went wrong with a pattern, an input or a replacement.

```tondo
import std.console
import std.regex

// Patterns a user typed, which only fail when the program runs.
for pattern in [r"(?<year>\d+)-(?<year>\d+)", r"\d{3"] {
    match regex.Regex.compile(pattern) {
        ok(_) => console.println("compiled")
        err(error) if error.kind == regex.RegexErrorKind.DuplicateCaptureName => {
            console.println("two groups have the same name, at byte {error.offset}")
        }
        err(error) => console.println("{error.kind}")
    }
}
```

Implements `Display`.

### RegexPhase

```tondo
pub enum RegexPhase {
    Compile
    Match
    Replace
}
```

When an error happened: compiling the pattern, matching or replacing.

```tondo
import std.console
import std.regex

let digits = regex.Regex.compile(r"\d+")?
match digits.replaceAll("a1b2", r"${missing}") {
    ok(text) => console.println(text)
    err(error) => {
        let when = match error.phase {
            regex.RegexPhase.Compile => "compiling"
            regex.RegexPhase.Match => "matching"
            regex.RegexPhase.Replace => "replacing"
        }
        console.println("failed while {when}: {error.kind}")
    }
}
```

Implements `Display`.

### RegexError

```tondo
pub type RegexError = {
    kind: RegexErrorKind
    phase: RegexPhase
    offset: Int
}
```

What went wrong, in which phase, at which byte of the pattern, input or
replacement.

```tondo
import std.console
import std.regex

fn search(pattern: String, input: String): Bool ! regex.RegexError {
    regex.Regex.compile(pattern)?.isMatch(input)?
}

match search(r"[a-", "abc") {
    ok(found) => console.println("{found}")
    err(error) => console.println("{error.kind} in {error.phase} at byte {error.offset}: {error}")
}
```

Implements `Display`.

### RegexSpan

```tondo
pub type RegexSpan = {
    start: Int
    end: Int
}
```

A half-open range of bytes of an input.

```tondo
import std.console
import std.regex

let input = "total: 42 EUR"
let amount = regex.Regex.compile(r"\d+")?.find(input)?.expect("a number")
let span = amount.span
console.println("bytes {span.start}..{span.end}: {span.slice(input)?}")
```

#### RegexSpan.slice

```tondo
pub fn RegexSpan.slice(self, input: String): String ! RegexError
```

The text of the span, which must fall between scalars of `input`.

### RegexCapture

```tondo
pub type RegexCapture = {
    span: RegexSpan
    text: String
}
```

A capture that took part in a match: where, and its text.

```tondo
import std.console
import std.regex

let date = regex.Regex.compile(r"(?<year>\d{4})-(?<month>\d{2})")?
let found = date.find("since 2026-11")?.expect("a date")
match found.captureName("month") {
    some(month) => console.println("month {month.text} at {month.span.start}")
    none => console.println("no month")
}
```

### RegexMatch

```tondo
pub type RegexMatch = {
    span: RegexSpan
    text: String
    // private fields
}
```

A match, with its captures; capture `0` is the whole match.

```tondo
import std.console
import std.regex

let pair = regex.Regex.compile(r"(\w+)=(\w+)")?
for found in pair.findAll("a=1 b=2")? {
    let item = found?
    let key = item.capture(1).map((capture) { capture.text }).unwrapOr("")
    let value = item.capture(2).map((capture) { capture.text }).unwrapOr("")
    console.println("{item.text}: {key} is {value}")
}
```

#### RegexMatch.capture

```tondo
pub fn RegexMatch.capture(self, index: Int): RegexCapture?
```

A capture by index, or `none` if it took no part or does not exist.

#### RegexMatch.captureName

```tondo
pub fn RegexMatch.captureName(self, name: String): RegexCapture?
```

A capture by name, or `none` if it took no part or does not exist.

### Regex

```tondo
pub type Regex = {
    // private fields
}
```

A compiled pattern, immutable and shared.

```tondo
import std.console
import std.regex

let email = regex.Regex.compile(r"(?<user>[\w.]+)@(?<host>[\w.]+)")?
console.println("{email.captureCount()} groups: {email.captureNames()}")
console.println("{email.isFullMatch("ada@example.com")?} {email.isMatch("no address")?}")
console.println(email.replaceAll("ada@example.com, bo@example.org", r"${user} at ${host}")?)
```

#### Regex.compile

```tondo
pub fn Regex.compile(
    pattern: String,
    options: RegexOptions = RegexOptions {
        caseInsensitive: false
        multiLine: false
        dotMatchesNewline: false
        crlf: false
        ungreedy: false
    },
    limits: RegexLimits = RegexLimits {
        maxPatternBytes: 65536
        maxSyntaxDepth: 256
        maxCaptureGroups: 256
        maxProgramStates: 1000000
        maxInputBytes: 16777216
        maxMatches: 1000000
        maxOutputBytes: 67108864
        maxReplacementBytes: 65536
    },
): Regex ! RegexError
```

Compiles a pattern; an error says what and where in it.

```tondo
import std.console
import std.regex

let date = regex.Regex.compile(r"(?<year>\d{4})-(?<month>\d{2})")?
match date.find("due 2026-11")? {
    some(found) => {
        let year = found.captureName("year").map((part) { part.text })
        console.println("{year} {found.text}")
    }
    none => console.println("no date")
}
```

#### Regex.pattern

```tondo
pub fn Regex.pattern(self): String
```

The pattern as written.

#### Regex.captureCount

```tondo
pub fn Regex.captureCount(self): Int
```

The numbered captures, without the whole match.

#### Regex.captureNames

```tondo
pub fn Regex.captureNames(self): Array[String]
```

The names of the named captures, in order.

#### Regex.isMatch

```tondo
pub fn Regex.isMatch(self, input: String): Bool ! RegexError
```

Whether the pattern matches anywhere in the input.

#### Regex.isFullMatch

```tondo
pub fn Regex.isFullMatch(self, input: String): Bool ! RegexError
```

Whether the pattern matches the whole input.

#### Regex.find

```tondo
pub fn Regex.find(self, input: String): RegexMatch? ! RegexError
```

The first match, or `none`.

#### Regex.findAll

```tondo
pub fn Regex.findAll(self, input: String): RegexFindIterator ! RegexError
```

The matches, in order and without overlapping.

#### Regex.replace

```tondo
pub fn Regex.replace(self, input: String, replacement: String): String ! RegexError
```

The input with the first match replaced: `$0` is the match, `$N` and
`${name}` its captures, and `$$` a `$`.

#### Regex.replaceAll

```tondo
pub fn Regex.replaceAll(self, input: String, replacement: String): String ! RegexError
```

The input with every match replaced, as `replace` does.

#### Regex.replaceWith

```tondo
pub fn Regex.replaceWith(
    self,
    input: String,
    replacement: fn(RegexMatch): String,
): String ! RegexError
```

The input with the first match replaced by what `replacement` gives for
it, as it is: a `$` in it is a `$`.

#### Regex.replaceAllWith

```tondo
pub fn Regex.replaceAllWith(
    self,
    input: String,
    replacement: fn(RegexMatch): String,
): String ! RegexError
```

The input with every match replaced by what `replacement` gives for it,
as `replaceWith` does.

#### Regex.escape

```tondo
pub fn Regex.escape(text: String): String
```

A pattern that matches `text` as it is, with each character the syntax
gives a meaning escaped.

### RegexFindIterator

```tondo
pub type RegexFindIterator = {
    // private fields
}
```

The matches of `findAll`, found as they are asked for.

```tondo
import std.console
import std.regex

let words = regex.Regex.compile(r"\w+")?
var count = 0
for found in words.findAll("the quick brown fox")? {
    count += 1
    if found?.text == "brown" {
        break
    }
}
console.println("{count} words up to brown")
```

Implements `Iterator[RegexMatch ! RegexError]`.
