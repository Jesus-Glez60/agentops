Extract what the user actually wants before any plan, spec, or code exists —
not what they think they should ask for. Use this when a request is missing
who it's for, why now, what success looks like, or the binding constraint;
when the ask is conventional rather than specific ("build me X", "make it
faster"); or when the user explicitly asks to be interviewed or
stress-tested. Skip it for unambiguous, self-contained changes (renames,
typo fixes, pure information requests), and don't run it in non-interactive
contexts (CI, `/loop`, autonomous runs) — flag the ambiguity as a blocker
instead of guessing.

Before asking anything, write down your current best read of what the user
wants in one sentence plus an honest confidence number (0-100%). Ask one
question at a time, each with your own guess attached and the reasoning
behind it — never a batch; the user reacts faster to a wrong guess than
they generate an answer from scratch, and batching lets them skim-read past
what matters. Ask via the `AskUserQuestion` tool when it's available, with
2-4 concrete options reflecting realistic answers (not generic yes/no
placeholders) — the user can always type something else. After each
answer, briefly acknowledge the decision (1-2 sentences) before moving to
the next question; don't let acknowledgments stack into a running summary.
Watch for answers that pattern-match best-practice talk ("scalable", "the
standard approach") rather than naming a specific outcome; when you hear
one, ask what they'd actually want if they didn't have to justify it to
anyone.

Stop when you can predict the user's reaction to the next three questions
you'd ask — that's the test, not a fixed number of rounds. Then restate the
intent in the user's own words, tight (5-8 lines): outcome, user, why now,
success, constraint, and — non-negotiable — what's explicitly out of
scope. The gate is an explicit yes; "whatever you think," "sounds good,"
and silence followed by "okay let's start" are not yes. Loop on corrections
until you get one.

Once confirmed, deliver the statement of intent, offer to hand off to
`idea-refine` (if it still needs variations) or `spec-driven-development`
(if it's concrete enough to specify), and stop your turn — do not start
downstream work in the same turn the user just confirmed.

**Plan-flow mode.** When you're called from the `plan` skill (the
task-start flow), the plan is the next step, not a separate turn. Use the
same trigger and the same one-question-with-a-guess style, but:

- Ask at most 3 questions (`MAX_PLAN_QUESTIONS`), only about the outcome,
  the scope boundary, or a constraint the plan depends on, and only what
  recall and the code didn't answer.
- Skip the closing statement of intent and the explicit-yes gate. Don't
  stop the turn: hand the answers back to `plan` and keep planning in the
  same turn.
- Anything still unanswered when you stop becomes an "Open assumptions"
  entry at the top of the plan. Approving the plan is the user's chance to
  correct it.
