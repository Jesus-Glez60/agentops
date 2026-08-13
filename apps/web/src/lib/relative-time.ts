// Ported from main's shelved dashboard (lib/relative-time.ts). Both
// timestamp forms are genuinely needed here, not speculative:
// relativeTimeFromUnixSeconds for /repos' manifest-sourced timestamps,
// relativeTimeFromIsoString for /activity's ScanHistory-sourced ones.
export function relativeTimeFromMs(ms: number): string {
  const diffMs = Date.now() - ms;
  // Math.floor, not Math.round: "how many whole units have fully elapsed"
  // -- rounding would show 30s-ago as "1m ago", which reads as wrong.
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 1) return "just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  return `${Math.floor(diffHours / 24)}d ago`;
}

export function relativeTimeFromUnixSeconds(unixSeconds: number): string {
  return relativeTimeFromMs(unixSeconds * 1000);
}

export function relativeTimeFromIsoString(iso: string): string {
  // SQLite's `CURRENT_TIMESTAMP` (the source of ScanHistory.started_at)
  // produces real UTC time as "YYYY-MM-DD HH:MM:SS" -- no 'T', no 'Z'. JS's
  // Date constructor parses that shape leniently as *local* time instead of
  // UTC, which would silently skew every relative-time label by the
  // browser's UTC offset. Normalize to real ISO-8601 UTC first; a string
  // that's already proper ISO (has 'T') passes through unchanged.
  const normalized = /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/.test(iso) ? `${iso.replace(" ", "T")}Z` : iso;
  return relativeTimeFromMs(new Date(normalized).getTime());
}
