/**
 * Runs `fn` over `items` with at most `limit` in flight at once, returning
 * results in the same order as `items` (same shape `Promise.allSettled`
 * gives). Exists because firing every item's request simultaneously (a
 * bare `Promise.all(items.map(fn))`) breaks under real load for this
 * backend specifically: every request opens its own fresh Postgres
 * connection pool (`agentops_mcp::open_store` per-request, not a shared
 * pool on `AppState` -- a real backend inefficiency, tracked separately,
 * too invasive to fix blind here) rather than reusing one, so a burst of
 * concurrent requests is a burst of concurrent pool creations. Confirmed
 * live: the Gotchas page's "Keep all" against 54 gotchas failed 32 of
 * them under full concurrency; this caps the burst instead.
 */
export async function mapWithConcurrency<T, R>(items: T[], limit: number, fn: (item: T, index: number) => Promise<R>): Promise<PromiseSettledResult<R>[]> {
  const results: PromiseSettledResult<R>[] = new Array(items.length);
  let next = 0;

  async function worker() {
    while (next < items.length) {
      const index = next++;
      try {
        results[index] = { status: "fulfilled", value: await fn(items[index], index) };
      } catch (reason) {
        results[index] = { status: "rejected", reason };
      }
    }
  }

  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return results;
}
