# ADR 0003: Insertion-only shell rewrite

- Status: accepted
- Date: 2026-08-02

## Decision

Parse Bash into a concrete syntax tree, locate executable byte spans, and insert a constant governor
prefix. Never reconstruct or normalize the whole shell string.

## Consequences

Supported commands preserve quoting, redirects, operators, and whitespace. Dynamic or ambiguous
syntax passes unchanged. Coverage expands through corpus fixtures, not optimistic parsing.
