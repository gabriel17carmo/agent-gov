# Release readiness and evidence

This page separates automated evidence from claims that require a physical Mac or external signing
credentials. Passing CI is necessary, but it is not treated as proof of real-world responsiveness.

## Automated gates

| Area | Evidence | Status |
|---|---|---|
| Formatting and static analysis | rustfmt, clippy with warnings denied, and actionlint in `CI` | Automated |
| Cross-platform behavior | Rust tests on Linux, macOS arm64, and macOS Intel | Automated |
| Scheduler limits | Multi-process capacity, queue overflow, quarantine, and signal tests | Automated |
| Hook safety | Claude/Cursor fixtures, unknown-field preservation, RTK failure modes, and property tests | Automated |
| Cancellation identity | Exact job lookup, held slot lock, and private control-socket integration test | Automated |
| Terminal path | Pseudo-TTY smoke test on both hosted Mac architectures | Automated |
| Installation | Transactional installer fixtures and macOS smoke tests | Automated |
| Dependencies | Weekly advisory and license checks plus Dependabot | Automated |
| Parser robustness | Fuzz targets compile on pull requests and run on a weekly schedule | Automated |
| Distribution | Universal binary, checksum, SBOM, generated notes, and provenance attestation | Automated |

## Physical-Mac gates

These items must not be marked complete from hosted CI alone:

- real Claude Code, Cursor, subagent, and RTK versions recorded in the compatibility matrix;
- A/B benchmark with four to six agents at capacity `1` and `2`;
- typing, window-switching, memory-pressure, swap, and endpoint-product observations;
- sleep/wake, clock changes, Gradle daemon behavior, and a 24-hour stress run;
- physical IDE-terminal behavior beyond the pseudo-TTY smoke test;
- identity-verified recovery of a live orphan process group;
- strict rejection of unsupported network filesystems.

Use [the benchmark protocol](benchmark.md) and attach only aggregated, redacted measurements. Never
commit corporate commands, transcripts, environment variables, or endpoint logs.

## External credentials and repository settings

- Apple Developer ID signing and notarization require a certificate, private key, Apple account
  credentials, and explicit secret configuration. Releases remain checksummed and attested until
  those credentials are available.
- A Homebrew tap requires a separately maintained tap repository after the release format is stable.
- The repository owner should enforce a `main` ruleset with pull requests, required `CI` and
  `Security` checks, blocked force-push/deletion, and private vulnerability reporting.

## Promotion rule

The project can remain a public preview while automated gates are green. Promotion to `v1.0`
requires the physical-Mac gates, a documented capacity decision, and signed/notarized distribution
or an explicitly accepted release exception.
