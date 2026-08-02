# ADR 0002: Stable kernel locks without a daemon

- Status: accepted
- Date: 2026-08-02

## Decision

Use one stable advisory lock file per enabled slot and a short-held queue lock. The active supervisor
owns exactly one slot and keeps running until its child exits. Do not ship a daemon in the MVP.

## Consequences

Crash cleanup relies on kernel descriptor closure. Queue fairness is approximate and bounded. A
daemon may be reconsidered only if weighted pools, strict fairness, or coalescing are measured needs.
