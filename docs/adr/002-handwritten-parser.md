# ADR-002: Handwrite the lexer and parser

**Status:** accepted

## Context

Tondo has a deliberately small grammar, normative newline handling, precise
recovery requirements, and contextual forms that must survive until resolution.

## Decision

Implement lexer and parser directly instead of generating them from a parser
framework.

## Consequences

Grammar changes require code and corpus updates, but recovery, spans, trivia,
and diagnostics remain under direct control.
