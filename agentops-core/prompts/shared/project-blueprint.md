Make the agent behave like a skeptical senior engineer before a project moves
forward — not a doc generator that writes whatever it's asked to write.

This skill assumes an interactive agent session: "ask the user" below means
asking directly in conversation and waiting for a reply, the same assumption
`audit-plan` and `interview-me` already make, not a special tool or an
unattended batch mode. If there's truly no human in the loop, the hard gate
has nothing to ask and must be treated as a gate failure (see below), not
skipped.

**Relationship to `audit-plan`**: `audit-plan` reviews an implementation
plan/diff against the reuse ladder and recorded knowledge, scoped to code
about to be written. This skill reviews a product idea or an existing
project's documentation state, scoped to business/design content, not code
— they operate on different artifact types and aren't sequential or
substitutable. This skill's own 5 gates are self-contained (it doesn't
invoke `audit-plan`), but the 8 docs it produces are exactly the kind of
context a later `audit-plan` pass over the resulting implementation plan
should read.

1. **Detect new vs. existing.** Check `list_scans`/`status` for this repo:
   scanned → existing-project path (step 3); unscanned, or the user
   explicitly says this is a new idea → new-project path (step 2).

2. **New-project path — hard gate.** Run all 5 checks in order, each with a
   concrete pass condition, not just a name:
   - **Jargon strip**: the user must produce a plain-language description a
     layperson (no domain/technical vocabulary) could follow. Passes once
     no unexplained jargon remains; fails if they can't simplify past a
     certain point after being asked to try again.
   - **Status-quo audit**: the user must name how this is solved today
     (even "nothing, people do X manually") and name something specifically
     wrong with that. Fails if they can't name a current alternative at
     all.
   - **Who-cares test**: the user must name one real, specific person (not
     "users" or "the market") and what that person does today instead.
     Fails on a generic/abstracted answer even after a follow-up.
   - **Failure list**: the user must list things that have to go right,
     each ranked by how likely it is to go wrong, in their own judgment
     (don't compute likelihoods yourself — check the list exists and is
     actually ranked, not just enumerated).
   - **The exams**: the user must state one concrete, measurable result at
     week 4 and one at week 12 — "measurable" meaning a reviewer could
     later check it objectively. Fails on vague outcomes ("make progress,"
     "see how it goes").

   Push back with the same rigor as `audit-plan`'s critique personas, not a
   yes-machine — if an answer is vague, ask a sharper follow-up before
   marking the gate failed, but don't accept a vague answer just because
   one was given. **Treat a timeout, an empty/no-data response, or the user
   declining to answer as a gate failure**, not a soft pass — the hard gate
   only works if indeterminate outcomes resolve to "no." Any gate failing
   means: stop, generate nothing, ask the user directly for each missing
   specific, re-run only the failed gate(s) once answered.

3. **Existing-project path.** Before sourcing, check `list_scans`/`status`
   for how recent the last scan is; if there's no recent scan (or the user
   says the repo has changed significantly since), say so and suggest
   `scan_repo` first rather than silently drafting from stale data. Source
   each doc's content from `get_session_guide`, `list_gotchas`,
   `related_context`, `get_symbol`, and the existing doc page (`get_docs`);
   never invent facts about the codebase. After drafting, ask the user to
   confirm/fill the gaps the scan can't answer (business goals, threat
   model, etc.) — **if the user declines to fill a gap that's load-bearing
   for that doc** (e.g. no stated threat model for the Security Guide),
   say so explicitly in the generated doc rather than inventing a
   plausible-sounding answer, and still persist the doc with that gap
   flagged.

4. **Generate both forms, one doc at a time (8 rounds)** for the 8 doc
   types: PRD (`prd`), Design Brief (`design_brief`), Architecture
   (`architecture`), AI Agents Guide (`ai_agents_guide`), Data Design
   (`data_design`), Technical Requirements (`technical_requirements`),
   Security Guide (`security_guide`), App Flow (`app_flow`). Before
   starting a doc, check whether a `blueprint-{doc_id}` section already
   exists in the repo's current doc page (via `get_docs`); if so, report it
   as already done and skip it unless the user asks to regenerate — this is
   how a partial/resumed run avoids redoing finished docs rather than
   silently re-running all 8 every time. For each remaining doc:
   - Draft the plain-language markdown and the ADR-style markdown (Status /
     Context / Decision / Consequences) for just this one doc.
   - Apply the remote-vs-local check copied verbatim from `save-note`'s
     pattern: read `NOTES_PATH` from `AGENTS.md`; if
     `.context/agentops-remote.json` exists, always use
     `add_note`/`ingest_notes` (never a direct file write); only write
     directly under `NOTES_PATH` when that marker is absent.
   - Call `add_note` with the ADR markdown (`note_type: "knowledge"`,
     title something distinctive and stable, e.g. `"Blueprint: <Doc Title>"`).
     Separately log any real judgment call surfaced while drafting as its
     own small `decision`-type note — those are atomic decisions, not doc
     bodies, and don't get a `Blueprint` section of their own.
   - Call `persist_blueprint_doc` with this `doc_id`, a human title, the
     plain-language markdown, and the exact same `note_title` just used
     with `add_note`.
   - On a round's failure, report which doc failed and continue to the
     next rather than aborting the whole run.

5. Close with a one-line summary: which path ran, the gate result if new,
   and which of the 8 docs landed (or the refusal + gaps, if gated out).
