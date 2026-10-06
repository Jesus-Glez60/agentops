Produce an evidence-backed spec for a feature inspired by an existing app the
user has no source for — never application code itself.

Confirm REA is reachable before anything else: attempt one REA tool call
(e.g. `binary_session`). If it errors or the tool isn't found, stop and tell
the user to run `npx -y rea-agents@latest setup` themselves — don't register
it on their behalf, since `setup` edits global tool configs
(`~/.claude.json`, `~/.cursor/mcp.json`, etc.) shared across every project,
not just this one, and needs their explicit approval for that scope. If
reachable but `doctor` reports a stale registration, surface that and ask
before proposing `setup` again.

Once reachable, load REA's own installed skill
(`~/.agents/skills/reverse-engineer-anything/SKILL.md`, registered by
`rea setup`) for the actual mechanics — target routing (`open_binary` vs.
`analyze_javascript_application` vs. `list_browser_targets`, etc.), its
staged-investigation workflow, and its evidence-citation rules
(`confidence: observed|derived|inferred`, `authority`, residual-unknown
tracking). Don't re-derive that guidance here; it's REA's own, already
installed, and more current than anything duplicated into this file.

Follow REA's own skill through investigation and evidence-gathering. This
skill picks up specifically where that one leaves off:

1. Once REA's own finding-ledger step is complete, write the spec as a
   confidence-labeled document: one section per sub-feature, each claim
   tagged with its REA confidence tier, the evidence IDs behind it, and any
   residual unknowns REA's `list_unknowns`/`build_reconstruction_obligation_ledger`
   surfaced. Never upgrade an `inferred` finding to a stated fact in the
   spec — carry the tier through.
2. Keep the spec scoped to *what the feature does*, not *how to implement
   it in this codebase* — that's the next skill's job. Hand off to
   `spec-driven-development` (if no spec exists yet for the target project)
   or `plan` (if one does) to turn this into an implementation plan. This
   skill's own output is the spec, never application code.
3. If anything in the investigation surfaced a real, reusable finding about
   REA itself (a systematic blind spot, a provider limitation, a confidence
   tier that turned out miscalibrated), record it via the `save-note` skill
   — not just left in this session's own output.

## Calibrating trust in REA's confidence tiers (do this once, before relying on it for a real no-source target)

REA is built for targets with no available source. Before trusting its
`inferred`-tier findings on a genuine no-source target, run it once against
a target where the real source is independently known, and diff its output
against that ground truth — this project already did this once against
`github.com/oraios/serena` and `github.com/headroomlabs-ai/headroom`, whose
real architecture is recorded in this project's own notes (search
`related_context`/`list_gotchas` for "headroom" or "serena" before repeating
this from scratch).

This calibration is **partial and directional, not a substitute for
validating against a genuine no-source target** — grading REA where the real
source is always available as a tiebreaker only shows whether its reported
confidence tiers roughly track reality on an easy case; it says nothing
about its accuracy when no such tiebreaker exists, which is the actual
target use case. Treat a clean calibration result as "safe for a first
exploratory pass," not as "fully trusted."

To (re-)run the calibration: clone the target repo to a scratch directory
(`github.com/oraios/serena` or `github.com/headroomlabs-ai/headroom`); if the
clone fails or the repo is unreachable, skip that target, say so explicitly,
and don't block on it. Ask REA a scoped question whose real answer is
already known (e.g. "how does Serena's symbol cache persist across
sessions" — real answer: Python pickle, not portable) and diff REA's
confidence-tagged output against the known answer. Record the result (trust
level per confidence tier, any systematic blind spot found, which targets
could or couldn't be validated) via `save-note`, explicitly marked
partial/directional.
