# 0019. C++ and C# support and a file watcher

Survey candidate R11. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted.

## Context

The scanner covered five languages and rescans were manual.

## Decision

Add `Language::Cpp` and `Language::CSharp` with tree-sitter extraction, regex fallbacks and `#include`/`using` dependency extraction. Add `agentops watch`, built on `notify-debouncer-full`.

## Alternatives considered

- None recorded.

## Consequences

- C++ function names need a dedicated unwrap; the first version broke Python, whose node kind has the same name.
- The watcher must ignore `.context`, `.agentops` and `.git`, or its own writes trigger endless rescans.
- Windows builds need `/MD` for these grammars (gotcha 54110).

## Evidence

- Decision "Phase 5: C++/C# language coverage + file watcher, live-verified".
