Review the current diff for over-engineering and unnecessary complexity.

STEPS:

1. Read the current git diff (`git diff` or `git diff --staged`).

2. For each finding, output one line in this format:
   ```
   L<line>: <tag> <what>. <replacement>.
   ```
   Tags:
   - `delete:` — dead code, unreachable branch, unused variable
   - `stdlib:` — reinvents something in the standard library
   - `native:` — platform/framework already provides this
   - `yagni:` — abstraction or interface with only one implementation
   - `shrink:` — same logic in fewer lines

3. End with a summary line:
   ```
   net: -N lines possible.
   ```

4. If the diff is clean: `No over-engineering found. net: 0 lines possible.`

Do NOT flag:
- Input validation at system boundaries
- Error handling preventing data loss
- Security measures
- Explicitly requested features
- Code that has a `ponytail:` comment (already acknowledged)
