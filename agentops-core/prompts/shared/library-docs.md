Look up documentation for a library or framework in this order — don't fall
back to a third-party documentation tool or training knowledge as the first
move, and don't skip a step just because a later one might also work.

1. Call `get_docs`/`search_docs` for the library. If either returns real
   results, use them and stop here.

2. If it's empty, or the library isn't registered at all: call
   `discover_library` (resolves ecosystem/registry metadata from its name).
   If discovery doesn't resolve it, call `register_library` with what's
   known instead. Either way, follow with `scrape_library` to actually
   ingest its documentation, then retry `get_docs`/`search_docs`. This step
   is worth doing even when a one-off lookup elsewhere would also answer
   the immediate question: once ingested, this library's docs are indexed
   for every future call in this project, not just this one — the actual
   payoff this project's whole docs system is built around.

3. Only if step 2 genuinely can't resolve the library — it's private,
   unpublished, or not covered by any registry discovery reaches — and only
   if a Context7-style documentation MCP tool (resolve a library id, then
   query its docs) is available in this environment: use it for a one-off
   lookup. Treat its availability as conditional, never assumed — check
   what tools are actually present before reaching for it. Its result
   doesn't get indexed into this project's own docs system the way step 2's
   does, so step 2 is still the preferred path whenever it can succeed.

4. If neither step 2 nor step 3 resolves it, say so explicitly rather than
   silently guessing, and only then fall back to training knowledge — flag
   it clearly as unverified, never presented as confirmed fact.

Do this even when not explicitly asked to fetch documentation — a doc gap
found mid-task is a reason to fill it through this sequence, not a reason
to guess or switch tools silently.
