# Contributing

1. Open an issue describing the requirement, compatibility gap, or measured failure.
2. Map the change to an FR/NFR or add an ADR for a new architectural decision.
3. Add a regression fixture or property/integration test with the implementation.
4. Run format, clippy, and all tests.
5. Keep command examples and fixtures redacted; never commit corporate transcripts or endpoint logs.

Pull requests should explain the safety boundary affected, fail-open/fail-closed behavior, test
evidence, and macOS architectures exercised. New dependencies require a short justification and must
pass license/advisory checks.

Do not broaden the MVP to root privileges, daemons, remote execution, endpoint-security changes, or
project-local trusted configuration without an accepted ADR.

Workflow changes must pass `scripts/check-workflows.sh` and `scripts/test-release-plan.sh`. Changes
to hook payload handling or shell parsing must keep the fuzz targets building; scheduled runs own
longer fuzz campaigns and must never upload proprietary corpora.
