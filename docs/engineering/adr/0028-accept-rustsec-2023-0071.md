# 0028. Accept RUSTSEC-2023-0071 for GitHub App signing

Survey candidate S10. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, with the merge (2026-08-24).

## Context

When the heavy crates joined the root workspace, `cargo deny` flagged the `rsa` timing side channel, pulled in through `jsonwebtoken` for GitHub App RS256 JWTs.

## Decision

Ignore the advisory in `deny.toml`, with the reason documented. The author was asked and chose to accept it.

## Alternatives considered

- Switch algorithms. Impossible: GitHub App auth requires RS256.
- Upgrade `rsa`. No patched release exists.

## Consequences

- The process signs its own JWTs locally and is not a queryable oracle, which narrows but does not remove the risk.
- Revisit when RustCrypto ships a constant-time fix.

## Evidence

- Decision "RUSTSEC-2023-0071 (RSA Marvin Attack timing side-channel) accepted, not fixed -- required for GitHub App RS256".
- Comment in `deny.toml`.
