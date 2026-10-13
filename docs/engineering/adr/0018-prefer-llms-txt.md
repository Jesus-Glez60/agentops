# 0018. Prefer llms.txt when crawling library docs

Survey candidate R10. Rationale is taken only from the sources under "Evidence"; nothing here is inferred.

## Status

Accepted, 2026-08-13 (`11ba058`).

## Context

A registered `docs_url` is often a marketing homepage. Scraping its HTML links pulled in navigation and marketing text.

## Decision

When `max_pages > 1`, look for `llms.txt` beside the docs URL, at the root, and at `/docs/llms.txt`, and crawl its links. If it links to a more specific `llms.txt`, follow that one hop. Fall back to same-origin HTML links otherwise.

## Alternatives considered

- HTML link-scraping only. Kept as the fallback.
- Unbounded `llms.txt` recursion. Rejected; one hop only.

## Consequences

- Next.js docs come back without blog pages (regression test).

## Evidence

- Decision "docbrain's shallow crawl now prefers a site's llms.txt over blind HTML link-scraping -- including following a meta-index one hop deeper".
