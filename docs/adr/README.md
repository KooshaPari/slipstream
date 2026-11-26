# Architecture Decision Records

This directory holds Slipstream architecture decision records. Each ADR is immutable
once Accepted and is superseded rather than edited.

| Number | Title | Status | Date | Path |
|---|---|---|---|---|
| 0001 | Desktop dashboard framework | Accepted | 2026-11-26 | 0001-desktop-dashboard-framework.md |

## Supporting research

Supporting research lives in `docs/adr/research/`. The index for that directory is
`docs/adr/research/README.md`.

## Superseding an ADR

To change a decision, write a new ADR that states which ADR it supersedes and why.
Update the old ADR's `Superseded by` field to point at the new one. Never edit the
body of an Accepted ADR.