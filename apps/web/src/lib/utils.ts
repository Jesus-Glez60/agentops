import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/** Backend `repo` strings (`ActivityEvent.repo`/`GotchaSummary.repo`/`SearchResult.repo`/`NodeDetail.repo`) are `<connection_id>--<32-hex-char-tenant-id>` since `checkout_path` (agentops-heavy-api's Rust source of truth) must embed the tenant to stay collision-free across tenants sharing a connection id -- see that function's doc comment. Strips the tenant suffix for human display only; the full string is still what identity comparisons (`selected?.repo === x.repo`) and SWR keys use, unchanged. */
export function displayRepoName(repo: string): string {
  return repo.replace(/--[0-9a-f]{32}$/, "")
}
