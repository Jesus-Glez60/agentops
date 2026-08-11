---
title: "Linear sync pull path live-verified against a real project (CurrentYachts)"
type: decision
---

agentops-linear's pull_issues was live-verified against a real Linear API key and the CurrentYachts project (2026-08-11): pulled 10 real issues via the GraphQL query, correctly mapped state.type completed to TaskStatus::Done, and confirmed idempotent (re-pulling with the same limit produced no duplicates, upsert_external_task worked correctly against real data). Push (sync_push/issueUpdate) was deliberately NOT live-tested against this real board -- user declined, since it would mutate a real issue's state visible to their team, not something to do without explicit confirmation each time. Push logic is still only verified via wiremock. Full-suite testing against CurrentYachts and other real projects is planned for later once more of the roadmap is built.
