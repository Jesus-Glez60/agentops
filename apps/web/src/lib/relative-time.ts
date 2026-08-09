/** Generalizes the relative-time formatting that previously only existed
 * once, in the old repos/page.tsx -- now shared by anything that needs to
 * render a timestamp as "12m ago" (repo scans, connection creation times). */
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
  return relativeTimeFromMs(new Date(iso).getTime());
}
