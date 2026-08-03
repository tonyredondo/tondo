# TLF token-efficiency study

**Status:** exploratory evidence for `tondo-llm-form-draft`

**Date:** 2026-08-04

**Repository revision:** `0a64e86`

## Question

This study asks which reversible textual representation gives an LLM more
useful Tondo source per output token without introducing another language
semantics or requiring semantic inference in the decoder.

It is a lexical/token-count study, not yet a model-quality benchmark. The
adoption gate also requires generation, type checking, acceptance and repair
measurements as specified by `TLF-EVAL-001`.

## Corpus

The input was every `.to` source under `tests/`, `acceptance/` and `stdlib/`,
deduplicated by SHA-256 of its exact bytes:

| Measure | Value |
|---|---:|
| Physical sources read | 155 |
| Unique sources | 154 |
| Unique source bytes | 74,453 |

The corpus is implementation-heavy and test-heavy. It contains valid runtime
programs, compile-pass and compile-fail cases, scripts, testing examples and the
current meta companion. It does not represent future ecosystem code or prove
model accuracy.

Token boundaries and logical `NL` were obtained from the actual
`tondo-compiler` lossless lexer. No regex lexer was used to decide Tondo tokens.

## Tokenizers

The probe counted each source independently with:

| Name | Family / source |
|---|---|
| `o200k_base` | OpenAI `tiktoken` 0.11.0 |
| `cl100k_base` | OpenAI `tiktoken` 0.11.0 |
| `p50k_base` | OpenAI `tiktoken` 0.11.0 |
| `qwen2.5-coder` | `Qwen/Qwen2.5-Coder-7B-Instruct` tokenizer |
| `mistral-v0.3` | `mistralai/Mistral-7B-Instruct-v0.3` tokenizer |

OpenAI documents `tiktoken` as its reversible BPE tokenizer and exposes
`o200k_base`/`cl100k_base` as named encodings. Hugging Face Tokenizers documents
the normalization, pre-tokenization and subword-model pipeline used to load the
other public tokenizer artifacts.

Primary references:

- [OpenAI tiktoken](https://github.com/openai/tiktoken)
- [Hugging Face Tokenizers](https://huggingface.co/docs/tokenizers/)
- [Google SentencePiece](https://github.com/google/sentencepiece)

The matrix is intentionally heterogeneous. It is still not a promise about
closed provider tokenizers and must be refreshed before promotion.

## Candidates

### Source

Exact repository source, including indentation, physical layout and comments.

### Token tape

Ordinary comments and physical trivia removed, logical `NL` retained as `LF`,
and only the whitespace needed to avoid token merging reinserted.

### Semicolon tape

The token tape with logical `NL` encoded as `;`, allowing every other physical
line break to be trivia.

### TLF lexical

The selected candidate:

- all Tondo keywords, names, literals and operators retain their spelling;
- ordinary comments and redundant trivia are removed;
- logical `NL` becomes `;`;
- the optional leading `NL` immediately after `{` is omitted;
- the terminal separator is omitted and restored by the decoder;
- no keyword aliases or identifier dictionary exist.

### Keyword aliases

Frequent keywords replaced by one-letter aliases. This was tested to determine
whether shorter byte spellings are also cheaper tokens.

### Dense syntax

An exploratory second grammar that omitted named `fn`/`type` markers and used
new binding operators. It is not the selected format.

### Identifier dictionary

An optimistic greedy table for repeated identifiers with at least three uses
and eight characters. Candidate inclusion was chosen using the combined
tokenizer count, so this is an upper-bound experiment rather than an acceptable
canonical algorithm.

## Results

Counts are the sum of encoding each unique file separately. Percentages are
reductions from exact source for the same tokenizer.

| Variant | Bytes | `o200k_base` | `cl100k_base` | `p50k_base` | Qwen Coder | Mistral |
|---|---:|---:|---:|---:|---:|---:|
| Source | 74,453 | 22,034 | 22,016 | 26,030 | 23,616 | 28,540 |
| Token tape | 61,123 | 18,767 (14.8%) | 18,718 (15.0%) | 23,317 (10.4%) | 20,318 (14.0%) | 24,883 (12.8%) |
| Semicolon tape | 61,123 | 19,369 (12.1%) | 19,272 (12.5%) | 21,964 (15.6%) | 20,872 (11.6%) | 23,465 (17.8%) |
| **TLF lexical** | **60,631** | **18,870 (14.4%)** | **18,773 (14.7%)** | **21,472 (17.5%)** | **20,373 (13.7%)** | **22,973 (19.5%)** |
| Keyword aliases | 58,000 | 18,700 (15.1%) | 18,655 (15.3%) | 23,160 (11.0%) | 20,255 (14.2%) | 24,883 (12.8%) |
| Dense syntax | 58,736 | 18,222 (17.3%) | 18,169 (17.5%) | 23,128 (11.1%) | 19,769 (16.3%) | 24,566 (13.9%) |
| Dense + dictionary | 57,742 | 18,180 (17.5%) | 18,128 (17.7%) | 23,043 (11.5%) | 19,715 (16.5%) | 24,418 (14.4%) |

Across the five tokenizers, exact source used 122,236 aggregate tokens and TLF
lexical used 102,461, a 16.18% aggregate reduction. The unweighted mean of the
five per-tokenizer reductions is 15.97%.

The selected representation was not the fewest bytes. It was the strongest
aggregate token result among candidates that keep the Tondo token vocabulary
and avoid another declaration/expression grammar.

## Token probes

Representative counts explain why byte abbreviations do not reliably help:

| Text | o200k | cl100k | p50k | Qwen | Mistral |
|---|---:|---:|---:|---:|---:|
| `fn` | 1 | 1 | 1 | 1 | 2 |
| `f` | 1 | 1 | 1 | 1 | 2 |
| `import` | 1 | 1 | 1 | 1 | 2 |
| `i` | 1 | 1 | 1 | 1 | 2 |
| `return` | 1 | 1 | 1 | 1 | 2 |
| `r` | 1 | 1 | 1 | 1 | 2 |
| `let value =` | 3 | 3 | 3 | 3 | 4 |
| `value:=` | 2 | 2 | 3 | 2 | 4 |
| `calculateTotalPrice` | 3 | 3 | 5 | 3 | 4 |
| `@0` | 2 | 2 | 2 | 2 | 3 |

Replacing a familiar keyword by a letter usually changes bytes but not tokens.
Binding punctuation can save a token in some vocabularies and none in others.
Identifier references only become profitable when a long, multi-token name is
repeated enough to repay the dictionary header and reference cost.

The greedy identifier table selected 17 names in only 7 of 154 files. Its
incremental gain over the already-dense candidate was 0.2–0.5 percentage points
depending on tokenizer. That does not justify index tracking or a larger error
surface in the draft.

## Structural validation

The selected lexical transform was expanded back to Tondo and run through the
actual lexer/parser for all 154 unique sources. The sequence of lexical/syntax
diagnostic codes matched the original for 154/154 sources, including existing
negative fixtures.

This proves the spike preserved the parser classification on this corpus. It
does not replace the required CST-equivalence properties, source-map tests,
fuzzing or semantic differential execution of the production codec.

## Decision

`tondo-llm-form-draft` uses the lexical TLF candidate:

- Tondo token spellings remain unchanged;
- `;` carries logical `NL`;
- external whitespace is non-semantic;
- optional leading `NL` after `{` and the terminal separator are omitted;
- comments ordinary are omitted by canonical packing;
- documentation comments and shebang remain representable;
- no aliases, identifier tables, binary form or provider-specific profile are
  admitted.

This preserves almost all measured savings while minimizing grammar teaching,
decoder complexity and repair risk.

## Remaining evidence before stability

1. Implement a production encoder/decoder on the lossless CST.
2. Prove canonical and semantic round-trips with generated programs.
3. Compose byte-accurate source maps through the formatter.
4. Add negative corpus, fuzzing and resource-limit tests.
5. Measure first-pass correctness and total repair tokens across multiple
   models, not only tokenizer counts.
6. Include the prompt/grammar teaching overhead in short and long sessions.
7. Compare full-source generation with a separately specified structural patch
   protocol for edits.

No percentage in this document is a release promise. It is evidence for the
current draft decision and must be reproducible against the recorded corpus and
tool versions.
