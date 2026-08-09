import type { HealthStatus } from "@/components/shared/health-badge";
import type { ConnectionStatus, RepoSummary } from "@/lib/api/types";
import { isFailedStatus } from "@/lib/api/types";

// No server field says "this repo is stale" -- this is a client-computed
// threshold, not a port of any existing logic (repos/page.tsx's old health
// dot was unconditionally green, never real).
const STALE_THRESHOLD_SECONDS = 60 * 60 * 24 * 7; // 7 days

export function repoHealth(repo: RepoSummary, nowSeconds = Date.now() / 1000): HealthStatus {
  if (!repo.counts) return "not-indexed";
  if (nowSeconds - repo.last_scanned_at > STALE_THRESHOLD_SECONDS) return "stale";
  if (repo.counts.gotchas > 0) return "warning";
  return "healthy";
}

export function connectionHealth(status: ConnectionStatus): HealthStatus {
  if (status === "active") return "healthy";
  if (status === "pending") return "scanning";
  if (isFailedStatus(status)) return "failed";
  return "not-indexed";
}
