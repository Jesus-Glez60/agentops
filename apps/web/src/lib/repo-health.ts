import type { RepoConnection } from "@/lib/api/repos-api";

export type HealthStatus = "healthy" | "warning" | "stale" | "not-indexed";

// No server field says this repo is stale -- this is a client-computed
// threshold, ported from main's shelved dashboard (lib/repo-health.ts,
// tested there against 4 fixture cases). Its 7-day threshold and
// Warning-beats-Stale-beats-Healthy precedence are reused verbatim rather
// than guessed at again -- the design mock's own "3 days ago -> Stale" is
// illustrative mockup content, not a spec to reverse-engineer a number
// from. `scanning`/`failed` states from main's fuller 6-state union are
// intentionally not reproduced here -- those were always connection-registry
// states (GitHub App/SSH), out of scope for this local-repos-only pass.
const STALE_THRESHOLD_SECONDS = 60 * 60 * 24 * 7; // 7 days

/** `last_scanned_at` is optional (unlike the retired manifest-based
 * `RepoSummary`, `RepoConnection` has no equivalent timestamp exposed yet
 * by the tenant-scoped backend) -- when omitted, the staleness check is
 * skipped entirely rather than fabricating a bogus "recently scanned"
 * default. */
export function repoHealth(repo: Pick<RepoConnection, "counts"> & { last_scanned_at?: number }, nowSeconds: number = Date.now() / 1000): HealthStatus {
  if (!repo.counts) return "not-indexed";
  if (repo.last_scanned_at !== undefined && nowSeconds - repo.last_scanned_at > STALE_THRESHOLD_SECONDS) return "stale";
  // gotchas_needing_curation, not the raw gotchas total -- a gotcha is
  // permanent knowledge, never "resolved away", so keying "warning" off the
  // total meant it could never actually clear. Only the uncurated count is
  // something a curation action (Keep/Reduce, never a delete) brings to zero.
  if (repo.counts.gotchas_needing_curation > 0) return "warning";
  return "healthy";
}
