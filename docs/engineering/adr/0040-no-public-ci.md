# 0040. No public CI

Survey candidate T3. Rationale comes from the author's answer A-10 in `questions.md`.

## Status

Accepted. Recorded 2026-10-13.

## Context

`.github/workflows/` has only `release.yml`. Nothing in the repository runs the test suites on a pull request.

## Decision

There is no public CI. Contributors run `cargo test` and the web and marketing test suites themselves. A GitHub Actions test workflow may be added later.

## Alternatives considered

- None recorded.

## Consequences

- A pull request shows no automatic test result; reviewers run the suites.

## Evidence

- Author answer A-10, `questions.md`.
- `.github/workflows/release.yml`.
