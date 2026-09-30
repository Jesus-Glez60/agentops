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
  // ScanHistory.started_at comes from two different stores with two
  // different `::text` shapes: SQLite's `CURRENT_TIMESTAMP` produces real
  // UTC time as "YYYY-MM-DD HH:MM:SS" -- no 'T', no offset -- while
  // Postgres's `started_at::text` cast of a TIMESTAMPTZ column produces
  // "YYYY-MM-DD HH:MM:SS.ffffff+00" -- a space separator like SQLite's, but
  // *with* a trailing UTC offset, rendered bare (no colon, no minutes) for
  // whole-hour offsets. Neither shape is directly Date-parseable: blindly
  // appending "Z" to both (as this used to) breaks the Postgres case
  // ("...+00Z", two conflicting offsets at once -> NaN, the "NaNd ago" bug),
  // and a bare "+00" offset isn't valid per the Date Time String Format
  // either (it requires "+00:00"). Normalize both cases explicitly instead
  // of guessing with one blind append.
  const trimmed = iso.trim();
  if (!/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}/.test(trimmed)) return relativeTimeFromMs(new Date(trimmed).getTime());

  let normalized = trimmed.replace(" ", "T");
  const offsetMatch = normalized.match(/([+-]\d{2})(:(\d{2}))?$/);
  if (offsetMatch) {
    if (!offsetMatch[2]) normalized = `${normalized.slice(0, offsetMatch.index)}${offsetMatch[1]}:00`;
  } else {
    normalized += "Z";
  }
  return relativeTimeFromMs(new Date(normalized).getTime());
}
