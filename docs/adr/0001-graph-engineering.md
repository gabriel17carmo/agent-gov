# ADR 0001: Graph engineering for delivery

- Status: accepted
- Date: 2026-08-02

## Decision

Deliver Agent Governor as a directed graph of contract-bearing components and release gates. Use a
short implementation/test/review loop inside each node, but never treat loop completion as evidence
that the node is done.

## Rationale

Scheduler, parser, host protocols, RTK, and installation can be developed partly independently but
have safety dependencies. Explicit edges make fail-closed boundaries and release blockers visible.
