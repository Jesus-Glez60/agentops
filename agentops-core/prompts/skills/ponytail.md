Switch Ponytail minimalism mode or report the current one.

Usage: `/ponytail [lite|full|ultra|off]`

STEPS:

1. If no argument: report `Current Ponytail mode: full (default)` and show the 6-rung ladder summary.

2. If argument provided, switch mode and confirm:
   - `lite` — "Ponytail lite: I'll build what's asked and name the lazier alternative in one line."
   - `full` — "Ponytail full: the 6-rung ladder is enforced on all code proposals."
   - `ultra` — "Ponytail ultra: YAGNI extremist mode. I'll delete before adding and challenge requirements."
   - `off` — "Ponytail off: normal mode, no minimalism enforcement."

3. Show which non-negotiables are always active regardless of mode:
   - Input validation at system boundaries
   - Error handling preventing data loss
   - Security (OWASP Top 10)
   - Accessibility
   - Explicitly requested features
